//! The measured age of the RX packets at placement (dasio#128): our PTP
//! now less each packet's own stamp, so the transit, the RX pass's wake and
//! our clock's offset (dstack's `RxCounts::lateness_peak` since the device
//! started, and the heartbeat's last second's least and largest). Plain
//! information for the status JSON, dtray's line and the log: dasio never
//! computes, proposes or sets a latency from it. Every latency change is a
//! deliberate change the owner makes from dcontroller's long-term graph
//! (owner ruling 16.11.2026). Pure but for [`flows`], so tested
//! everywhere; the run loops feed it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::info;

/// `samples` at `rate` in ns, rounded up.
pub fn samples_ns(samples: u64, rate: u32) -> u64 {
    let rate = u128::from(rate.max(1));
    let ns = (u128::from(samples) * 1_000_000_000).div_ceil(rate);
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// One RX flow slot's figures as dstack keeps them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flow {
    /// Packets since the device started (0: the slot never received).
    pub packets: u64,
    /// The largest age at placement since the device started, samples.
    pub peak: u64,
    /// The largest age in the heartbeat's last second, samples.
    pub last_max: u64,
    /// The least age in the heartbeat's last second, ns; `None` for a
    /// second without a placed packet.
    pub last_min_ns: Option<u64>,
}

/// Each RX flow slot's figures of a running device.
pub fn flows(rx: &dstack::media::rx::RxShared) -> impl Iterator<Item = Flow> + '_ {
    (0..dstack::media::rx::MAX_FLOWS).map(|slot| {
        let counts = rx.lane(slot);
        Flow {
            packets: counts.packets,
            peak: counts.lateness_peak,
            last_max: rx.last_lateness(slot),
            last_min_ns: rx.last_min_lateness_ns(slot),
        }
    })
}

/// The device's figures, ns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// The largest age of any packet since start.
    pub peak_ns: u64,
    /// The last second's least and largest age; `None` when no slot placed
    /// a packet in it.
    pub last: Option<(u64, u64)>,
}

/// The device's figures from its slots at `rate`: the peak over the slots
/// that received, the last second over the slots that placed a packet in
/// it; `None` before any packet (an idle slot's 0 is no measurement).
pub fn measure(flows: impl IntoIterator<Item = Flow>, rate: u32) -> Option<Report> {
    let mut peak: Option<u64> = None;
    let mut last: Option<(u64, u64)> = None;
    for f in flows.into_iter().filter(|f| f.packets > 0) {
        let p = samples_ns(f.peak, rate);
        peak = Some(peak.map_or(p, |q| q.max(p)));
        if let Some(least) = f.last_min_ns {
            let most = samples_ns(f.last_max, rate).max(least);
            last = Some(last.map_or((least, most), |(a, b)| (a.min(least), b.max(most))));
        }
    }
    peak.map(|peak_ns| Report { peak_ns, last })
}

/// `ns` in ms, two places.
fn ms(ns: u64) -> String {
    format!("{:.2}", ns as f64 / 1e6)
}

impl Report {
    /// The log's sentence: a measurement, never advice.
    pub fn text(&self) -> String {
        match self.last {
            Some((least, most)) => format!(
                "RX arrival (measured packet age at placement): peak {} ms since start, last second {}-{} ms",
                ms(self.peak_ns),
                ms(least),
                ms(most),
            ),
            None => format!(
                "RX arrival (measured packet age at placement): peak {} ms since start, no packet in the last second",
                ms(self.peak_ns),
            ),
        }
    }

    /// dtray's line part.
    pub fn suffix(&self) -> String {
        match self.last {
            Some((least, most)) => format!(
                " · RX arrival {}-{} ms, peak {} ms",
                ms(least),
                ms(most),
                ms(self.peak_ns)
            ),
            None => format!(" · RX arrival peak {} ms", ms(self.peak_ns)),
        }
    }

    /// The status JSON's keys, each followed by a comma; the last second's
    /// only when it placed a packet.
    pub fn json(&self) -> String {
        let last = self.last.map_or_else(String::new, |(least, most)| {
            format!("\"rx_arrival_last_min_ns\":{least},\"rx_arrival_last_max_ns\":{most},")
        });
        format!("\"rx_arrival_peak_ns\":{},{last}", self.peak_ns)
    }
}

/// The status JSON's RX arrival keys: none before a measurement (never a
/// 0 that reads as a measured age).
pub fn json(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.json())
}

/// dtray's line part: nothing before a measurement.
pub fn suffix(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.suffix())
}

/// The figures shared with the status closure. The peak only grows, so a
/// slot whose counts dstack starts again never lowers it; the last second
/// is the latest look's. Each held as the value + 1, so 0 is "none".
#[derive(Debug, Clone, Default)]
pub struct Arrival(Arc<[AtomicU64; 3]>);

impl Arrival {
    /// Takes a look's figures (`None`: no packet yet).
    pub fn set(&self, r: Option<Report>) {
        let Some(r) = r else { return };
        let [peak, least, most] = &*self.0;
        peak.fetch_max(r.peak_ns.saturating_add(1), Ordering::Relaxed);
        let (l, m) = r
            .last
            .map_or((0, 0), |(l, m)| (l.saturating_add(1), m.saturating_add(1)));
        least.store(l, Ordering::Relaxed);
        most.store(m, Ordering::Relaxed);
    }

    /// The figures now; `None` before any packet.
    pub fn report(&self) -> Option<Report> {
        let [peak, least, most] = &*self.0;
        let peak_ns = peak.load(Ordering::Relaxed).checked_sub(1)?;
        let last = least
            .load(Ordering::Relaxed)
            .checked_sub(1)
            .zip(most.load(Ordering::Relaxed).checked_sub(1));
        Some(Report { peak_ns, last })
    }
}

/// The run loop's watch: folds each look into the shared [`Arrival`] and
/// says when the peak since start rose.
#[derive(Debug, Default)]
pub struct Watch {
    arrival: Arrival,
    said: Option<u64>,
}

impl Watch {
    /// The figures the status closure reads.
    pub fn arrival(&self) -> Arrival {
        self.arrival.clone()
    }

    /// Folds the slots' figures at `rate`; the report to log when the peak
    /// since start rose.
    pub fn observe(&mut self, flows: impl IntoIterator<Item = Flow>, rate: u32) -> Option<Report> {
        self.arrival.set(measure(flows, rate));
        let r = self.report()?;
        self.said.is_none_or(|s| r.peak_ns > s).then(|| {
            self.said = Some(r.peak_ns);
            r
        })
    }

    /// The figures now; `None` before any packet.
    pub fn report(&self) -> Option<Report> {
        self.arrival.report()
    }
}

/// Logs `r` at INFO: a measurement, never a warning (the latency is the
/// owner's decision).
pub fn log(r: &Report) {
    let (last_min_ns, last_max_ns) = r.last.unzip();
    info!(
        arrival_peak_ns = r.peak_ns,
        ?last_min_ns,
        ?last_max_ns,
        "{}",
        r.text()
    );
}

#[cfg(test)]
mod tests;
