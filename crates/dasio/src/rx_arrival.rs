//! The measured age of the RX packets at placement (dasio#128): our PTP
//! now less each packet's own stamp, so the transit, the RX pass's wake and
//! our clock's offset (dstack's `RxCounts::lateness_peak` since the device
//! started, the least and largest of the heartbeat's last second as the
//! 10 s look found it, and the mean over the last 10 s tick from two looks
//! at the slots' running totals). Since dstack v0.6.157 (dstack issue 350)
//! the age is the packet's at its ARRIVAL; the writer's wait to its read
//! (a bridge's ASIO switch pull) is reported apart as the read wait. Plain
//! information for the status JSON, dtray's line and the log: dasio never
//! computes, proposes or sets a latency from it. Every latency change is a
//! deliberate change the owner makes from dcontroller's long-term graph
//! (owner ruling 16.11.2026). Pure but for [`flows`], so tested
//! everywhere; the run loops feed it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::info;

/// `samples` at `rate` in ns, rounded up.
pub fn samples_ns(samples: u64, rate: u32) -> u64 {
    let rate = u128::from(rate.max(1));
    let ns = (u128::from(samples) * 1_000_000_000).div_ceil(rate);
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// A running total since the device started: a sum of ns over a count of
/// placed packets (dstack `RxCounts::lateness_sum_ns`/`lateness_count`,
/// `read_wait_sum_ns`/`read_wait_count`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// The ages (or waits) summed, ns.
    pub sum_ns: u64,
    /// The packets summed.
    pub count: u64,
}

impl Totals {
    /// The packets placed between an `earlier` look and this one. The sum
    /// is diffed modulo 2^64 (dstack's rule: it wraps only after 2^32
    /// packets at the 4.29 s cap). A count below the earlier one is a slot
    /// whose counts started again: all of this look is new.
    pub fn since(self, earlier: Totals) -> Totals {
        if self.count < earlier.count {
            self
        } else {
            Totals {
                sum_ns: self.sum_ns.wrapping_sub(earlier.sum_ns),
                count: self.count - earlier.count,
            }
        }
    }
}

/// The mean of `parts` taken together (weighted by packets, so a slot with
/// one packet weighs one packet), ns rounded down; `None` without a packet.
pub fn mean(parts: impl IntoIterator<Item = Totals>) -> Option<u64> {
    let (sum, count) = parts.into_iter().fold((0u128, 0u128), |(s, c), t| {
        (s + u128::from(t.sum_ns), c + u128::from(t.count))
    });
    (count > 0).then(|| u64::try_from(sum / count).unwrap_or(u64::MAX))
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
    /// The placed packets' arrival ages since the device started.
    pub age: Totals,
    /// Their waits from arrival to the writer's read since the device
    /// started.
    pub wait: Totals,
    /// The largest of those waits since the device started, µs.
    pub wait_peak_us: u64,
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
            age: Totals {
                sum_ns: counts.lateness_sum_ns,
                count: counts.lateness_count,
            },
            wait: Totals {
                sum_ns: counts.read_wait_sum_ns,
                count: counts.read_wait_count,
            },
            wait_peak_us: counts.read_wait_peak_us,
        }
    })
}

/// The writer's wait from a packet's arrival to its read (dstack issue
/// 350): at a bridge, the ASIO driver's buffer switch pull.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Wait {
    /// The mean over the last tick, ns; `None` for a tick without a placed
    /// packet.
    pub mean_ns: Option<u64>,
    /// The largest since start, µs.
    pub peak_us: u64,
}

/// The device's figures, ns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// The largest age of any packet since start.
    pub peak_ns: u64,
    /// The last second's least and largest age; `None` when no slot placed
    /// a packet in it.
    pub last: Option<(u64, u64)>,
    /// The mean age of the packets placed since the previous look (one
    /// counts tick, 10 s; the first look: since the device started);
    /// `None` when none was placed.
    pub mean_ns: Option<u64>,
    /// The read wait; `None` while no slot reported one.
    pub wait: Option<Wait>,
}

/// The device's figures from its slots at `rate` since the device started:
/// [`measure_since`] with no earlier look.
pub fn measure(flows: impl IntoIterator<Item = Flow>, rate: u32) -> Option<Report> {
    measure_since(flows, &[], rate)
}

/// The device's figures from its slots at `rate`, the means since the
/// `earlier` look at the same slots (a slot it lacks counts from zero):
/// the peak over the slots that received, the last second over the slots
/// that placed a packet in it, the means over the packets placed between
/// the looks; `None` before any packet (an idle slot's 0 is no
/// measurement).
pub fn measure_since(
    flows: impl IntoIterator<Item = Flow>,
    earlier: &[Flow],
    rate: u32,
) -> Option<Report> {
    let mut peak: Option<u64> = None;
    let mut last: Option<(u64, u64)> = None;
    let mut ages = Vec::new();
    let mut waits = Vec::new();
    let mut wait_peak: Option<u64> = None;
    for (slot, f) in flows.into_iter().enumerate().filter(|(_, f)| f.packets > 0) {
        let before = earlier.get(slot).copied().unwrap_or_default();
        ages.push(f.age.since(before.age));
        waits.push(f.wait.since(before.wait));
        if f.wait.count > 0 || f.wait_peak_us > 0 {
            wait_peak = Some(wait_peak.map_or(f.wait_peak_us, |w| w.max(f.wait_peak_us)));
        }
        let p = samples_ns(f.peak, rate);
        peak = Some(peak.map_or(p, |q| q.max(p)));
        if let Some(least) = f.last_min_ns {
            let most = samples_ns(f.last_max, rate).max(least);
            last = Some(last.map_or((least, most), |(a, b)| (a.min(least), b.max(most))));
        }
    }
    let mean_ns = mean(ages);
    let wait = wait_peak.map(|peak_us| Wait {
        mean_ns: mean(waits),
        peak_us,
    });
    peak.map(|peak_ns| Report {
        peak_ns,
        last,
        mean_ns,
        wait,
    })
}

/// `ns` in ms, two places.
fn ms(ns: u64) -> String {
    format!("{:.2}", ns as f64 / 1e6)
}

impl Report {
    /// The log's sentence: a measurement, never advice.
    pub fn text(&self) -> String {
        let mean = self.mean_ns.map_or_else(String::new, |m| {
            format!(", mean {} ms over the last tick", ms(m))
        });
        let wait = self.wait.map_or_else(String::new, |w| {
            let mean = w
                .mean_ns
                .map_or_else(String::new, |m| format!("mean {} ms, ", ms(m)));
            format!(
                "; read wait (arrival to the writer's read) {mean}peak {} ms since start",
                ms(w.peak_us.saturating_mul(1000))
            )
        });
        let base = match self.last {
            Some((least, most)) => format!(
                "RX arrival (measured packet age at placement): peak {} ms since start, last second at the 10 s look {}-{} ms",
                ms(self.peak_ns),
                ms(least),
                ms(most),
            ),
            None => format!(
                "RX arrival (measured packet age at placement): peak {} ms since start, no packet in the last second at the 10 s look",
                ms(self.peak_ns),
            ),
        };
        format!("{base}{mean}{wait}")
    }

    /// dtray's line part: the last second, the mean, the peak.
    pub fn suffix(&self) -> String {
        let mut parts = Vec::new();
        if let Some((least, most)) = self.last {
            parts.push(format!("{}-{} ms", ms(least), ms(most)));
        }
        if let Some(m) = self.mean_ns {
            parts.push(format!("mean {} ms", ms(m)));
        }
        parts.push(format!("peak {} ms", ms(self.peak_ns)));
        format!(" · RX arrival {}", parts.join(", "))
    }

    /// The status JSON's keys, each followed by a comma; the last second's
    /// only when it placed a packet.
    pub fn json(&self) -> String {
        let last = self.last.map_or_else(String::new, |(least, most)| {
            format!("\"rx_arrival_last_min_ns\":{least},\"rx_arrival_last_max_ns\":{most},")
        });
        let mean = self
            .mean_ns
            .map_or_else(String::new, |m| format!("\"rx_arrival_mean_ns\":{m},"));
        format!("\"rx_arrival_peak_ns\":{},{last}{mean}", self.peak_ns)
    }

    /// A bridge's status keys for its ASIO pull wait (the read wait at the
    /// driver's buffer switch), each followed by a comma; none while no
    /// slot reported one, the mean only for a tick with a packet.
    pub fn pull_wait_json(&self) -> String {
        self.wait.map_or_else(String::new, |w| {
            let mean = w.mean_ns.map_or_else(String::new, |m| {
                format!("\"rx_asio_pull_wait_mean_ns\":{m},")
            });
            format!("{mean}\"rx_asio_pull_wait_peak_us\":{},", w.peak_us)
        })
    }
}

/// The status JSON's RX arrival keys: none before a measurement (never a
/// 0 that reads as a measured age).
pub fn json(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.json())
}

/// A bridge's ASIO pull wait keys: none before a measurement.
pub fn pull_wait_json(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.pull_wait_json())
}

/// dtray's line part: nothing before a measurement.
pub fn suffix(r: Option<Report>) -> String {
    r.map_or_else(String::new, |r| r.suffix())
}

/// The latest look's figures beside the peak: the last second, the mean
/// over the tick and the read wait.
#[derive(Debug, Clone, Copy, Default)]
struct Tick {
    last: Option<(u64, u64)>,
    mean_ns: Option<u64>,
    wait: Option<Wait>,
}

/// The figures shared with the status closure. The peak only grows, so a
/// slot whose counts dstack starts again never lowers it (held as the
/// peak plus one, so 0 is "none"); so does the read wait's peak. The last
/// second and the means are the latest look's, under one lock so a reader
/// never sees one look's least with another's largest. Neither side is a
/// media thread.
#[derive(Debug, Clone, Default)]
pub struct Arrival {
    peak: Arc<AtomicU64>,
    tick: Arc<Mutex<Tick>>,
}

impl Arrival {
    /// Takes a look's figures (`None`: no packet yet).
    pub fn set(&self, r: Option<Report>) {
        let Some(r) = r else { return };
        self.peak
            .fetch_max(r.peak_ns.saturating_add(1), Ordering::Relaxed);
        if let Ok(mut tick) = self.tick.lock() {
            let wait = match (tick.wait, r.wait) {
                (Some(old), Some(new)) => Some(Wait {
                    mean_ns: new.mean_ns,
                    peak_us: old.peak_us.max(new.peak_us),
                }),
                (Some(old), None) => Some(Wait {
                    mean_ns: None,
                    peak_us: old.peak_us,
                }),
                (None, new) => new,
            };
            *tick = Tick {
                last: r.last,
                mean_ns: r.mean_ns,
                wait,
            };
        }
    }

    /// The figures now; `None` before any packet.
    pub fn report(&self) -> Option<Report> {
        let peak_ns = self.peak.load(Ordering::Relaxed).checked_sub(1)?;
        let tick = self.tick.lock().map(|t| *t).unwrap_or_default();
        Some(Report {
            peak_ns,
            last: tick.last,
            mean_ns: tick.mean_ns,
            wait: tick.wait,
        })
    }
}

/// The run loop's watch: folds each look into the shared [`Arrival`] and
/// says when the peak since start rose.
#[derive(Debug, Default)]
pub struct Watch {
    arrival: Arrival,
    said: Option<u64>,
    /// The previous look at each slot: the means' earlier totals (this
    /// reader's own window; never `take_max_lateness`).
    earlier: Vec<Flow>,
}

impl Watch {
    /// The figures the status closure reads.
    pub fn arrival(&self) -> Arrival {
        self.arrival.clone()
    }

    /// Folds the slots' figures at `rate`, the means since the previous
    /// look; the report to log when the peak since start rose.
    pub fn observe(&mut self, flows: impl IntoIterator<Item = Flow>, rate: u32) -> Option<Report> {
        let now: Vec<Flow> = flows.into_iter().collect();
        self.arrival
            .set(measure_since(now.iter().copied(), &self.earlier, rate));
        self.earlier = now;
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
    let read_wait_mean_ns = r.wait.and_then(|w| w.mean_ns);
    let read_wait_peak_us = r.wait.map(|w| w.peak_us);
    info!(
        arrival_peak_ns = r.peak_ns,
        ?last_min_ns,
        ?last_max_ns,
        arrival_mean_ns = ?r.mean_ns,
        ?read_wait_mean_ns,
        ?read_wait_peak_us,
        "{}",
        r.text()
    );
}

#[cfg(test)]
mod tests;
