//! The receiver latency the measured arrival asks for (dasio#128): each RX
//! flow slot reports the largest age of its packets at placement since the
//! device started (dstack's `RxCounts::lateness_peak`: our PTP now less the
//! packet's own stamp, so the transit, the RX pass's wake and our clock's
//! offset). A packet within the device's `latency_ns` is never late, so
//! that peak is what `latency_ns` must cover. dasio only REPORTS the
//! recommendation (log, status JSON, dtray's line) and never changes the
//! latency while it runs: the session sets it in device.toml at the next
//! restart (owner, 16.11.2026). Pure but for [`flows`], so tested
//! everywhere; the run loops feed it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{info, warn};

/// The headroom over the measured peak, ns. The peak is measured on the
/// clock and the RX path the pump reads from, so it already holds the
/// transit, the RX pass's wake and our clock's offset; the margin covers a
/// burst a little above the largest seen, and [`STEP_NS`]'s rounding adds
/// up to another step. A whole block (1 ms) would push every quiet PC past
/// the 1 ms dasio#128 aims at for nothing measured.
pub const MARGIN_NS: u64 = 250_000;

/// The recommendation's step, ns: rounded up to 0.25 ms.
pub const STEP_NS: u64 = 250_000;

/// `samples` at `rate` in ns, rounded up (a rounded-down age would
/// recommend a latency a fraction short of it).
pub fn samples_ns(samples: u64, rate: u32) -> u64 {
    let rate = u128::from(rate.max(1));
    let ns = (u128::from(samples) * 1_000_000_000).div_ceil(rate);
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// The largest arrival peak of the slots that received a packet, ns, from
/// each slot's `(packets, lateness_peak)` at `rate`; `None` before any
/// packet (an idle slot's 0 is no measurement).
pub fn peak_ns(flows: impl IntoIterator<Item = (u64, u64)>, rate: u32) -> Option<u64> {
    flows
        .into_iter()
        .filter(|&(packets, _)| packets > 0)
        .map(|(_, peak)| samples_ns(peak, rate))
        .max()
}

/// The latency the peak `peak_ns` asks for: the peak and [`MARGIN_NS`],
/// rounded up to [`STEP_NS`].
pub fn recommended_ns(peak_ns: u64) -> u64 {
    peak_ns
        .saturating_add(MARGIN_NS)
        .div_ceil(STEP_NS)
        .saturating_mul(STEP_NS)
}

/// Each RX flow slot's `(packets, lateness_peak)` of a running device.
pub fn flows(rx: &dstack::media::rx::RxShared) -> impl Iterator<Item = (u64, u64)> + '_ {
    (0..dstack::media::rx::MAX_FLOWS).map(|slot| {
        let l = rx.lane(slot);
        (l.packets, l.lateness_peak)
    })
}

/// dasio's own peak since start, shared with the status closure: it only
/// grows, so a slot whose counts dstack starts again never lowers it. Held
/// as the peak + 1, so 0 is "no packet yet" and `fetch_max` keeps it.
#[derive(Debug, Clone, Default)]
pub struct Peak(Arc<AtomicU64>);

impl Peak {
    /// Takes a look's peak (`None`: no packet in the look).
    pub fn seen(&self, ns: Option<u64>) {
        if let Some(ns) = ns {
            self.0.fetch_max(ns.saturating_add(1), Ordering::Relaxed);
        }
    }

    /// The peak since start, ns; `None` before any packet.
    pub fn get(&self) -> Option<u64> {
        self.0.load(Ordering::Relaxed).checked_sub(1)
    }

    /// The report against the configured `latency_ns`; `None` before any
    /// packet.
    pub fn report(&self, configured_ns: u64) -> Option<Report> {
        self.get().map(|p| Report::of(configured_ns, p))
    }
}

/// The configured latency against the measured peak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// The device's `latency_ns` (device.toml).
    pub configured_ns: u64,
    /// The largest packet age at placement since start, ns.
    pub peak_ns: u64,
    /// [`recommended_ns`] of the peak.
    pub recommended_ns: u64,
}

/// `ns` in ms, two places.
fn ms(ns: u64) -> String {
    format!("{:.2}", ns as f64 / 1e6)
}

impl Report {
    pub fn of(configured_ns: u64, peak_ns: u64) -> Self {
        Report {
            configured_ns,
            peak_ns,
            recommended_ns: recommended_ns(peak_ns),
        }
    }

    /// The configured latency is below the recommendation: a burst like
    /// the largest seen plays late (clicks).
    pub fn below(&self) -> bool {
        self.configured_ns < self.recommended_ns
    }

    /// The log's sentence.
    pub fn text(&self) -> String {
        if self.below() {
            format!(
                "RX latency {} ms is below the recommended {} ms (measured arrival peak {} ms since start): set latency_ns = {} in device.toml at the next restart",
                ms(self.configured_ns),
                ms(self.recommended_ns),
                ms(self.peak_ns),
                self.recommended_ns,
            )
        } else {
            format!(
                "RX latency {} ms holds: measured arrival peak {} ms since start, recommended latency_ns = {} ({} ms)",
                ms(self.configured_ns),
                ms(self.peak_ns),
                self.recommended_ns,
                ms(self.recommended_ns),
            )
        }
    }

    /// dtray's line part.
    pub fn suffix(&self) -> String {
        if self.below() {
            format!(
                " · RX {} ms < rec. {} ms",
                ms(self.configured_ns),
                ms(self.recommended_ns)
            )
        } else {
            format!(
                " · RX {} ms (rec. {})",
                ms(self.configured_ns),
                ms(self.recommended_ns)
            )
        }
    }

    /// The status JSON's keys, each followed by a comma.
    pub fn json(&self) -> String {
        format!(
            "\"rx_latency_ns\":{},\"rx_arrival_peak_ns\":{},\"rx_latency_recommended_ns\":{},\"rx_latency_below\":{},",
            self.configured_ns,
            self.peak_ns,
            self.recommended_ns,
            self.below(),
        )
    }
}

/// The status JSON's RX latency keys: none before a measurement (never a
/// 0 that reads as a measured age).
pub fn json(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.json())
}

/// dtray's line part: nothing before a measurement.
pub fn suffix(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.suffix())
}

/// The run loop's watch: folds each look into the shared [`Peak`] and says
/// when the recommendation changed.
#[derive(Debug)]
pub struct Watch {
    configured_ns: u64,
    peak: Peak,
    said: Option<u64>,
}

impl Watch {
    pub fn new(configured_ns: u64) -> Self {
        Watch {
            configured_ns,
            peak: Peak::default(),
            said: None,
        }
    }

    /// The peak the status closure reads.
    pub fn peak(&self) -> Peak {
        self.peak.clone()
    }

    /// Takes a look at the flows (`(packets, lateness_peak)` at `rate`);
    /// the report to log when the recommendation changed.
    pub fn look(&mut self, flows: impl IntoIterator<Item = (u64, u64)>, rate: u32) -> Option<Report> {
        self.peak.seen(peak_ns(flows, rate));
        let r = self.report()?;
        (self.said != Some(r.recommended_ns)).then(|| {
            self.said = Some(r.recommended_ns);
            r
        })
    }

    /// The report now; `None` before any packet.
    pub fn report(&self) -> Option<Report> {
        self.peak.report(self.configured_ns)
    }
}

/// Logs `r`: WARN when the configured latency is below the
/// recommendation, else INFO.
pub fn log(r: &Report) {
    if r.below() {
        warn!(
            latency_ns = r.configured_ns,
            arrival_peak_ns = r.peak_ns,
            recommended_ns = r.recommended_ns,
            "{}",
            r.text()
        );
    } else {
        info!(
            latency_ns = r.configured_ns,
            arrival_peak_ns = r.peak_ns,
            recommended_ns = r.recommended_ns,
            "{}",
            r.text()
        );
    }
}

#[cfg(test)]
mod tests;
