//! The DAW's turnaround: from a block's hand-over (the bridge publishes it)
//! until the driver returns its outputs (the header's ack), in ns. A block
//! whose turnaround passes one block period is counted late by the bridge
//! (its outputs are sent as silence). Measured to tell a slow DAW from a
//! late wake-up on the FOH PC (dasio#1).

use std::sync::atomic::{AtomicU64, Ordering};

/// Publish times of the newest blocks, by seq.
pub struct Stamps {
    slots: [(AtomicU64, AtomicU64); SLOTS],
}

const SLOTS: usize = 8;

impl Default for Stamps {
    fn default() -> Self {
        Stamps {
            slots: std::array::from_fn(|_| (AtomicU64::new(0), AtomicU64::new(0))),
        }
    }
}

impl Stamps {
    /// Block `seq` (from 1) handed over at `ns`.
    pub fn publish(&self, seq: u64, ns: u64) {
        let (s, t) = &self.slots[seq as usize % SLOTS];
        // The seq last: a reader that sees it sees the time.
        s.store(0, Ordering::Release);
        t.store(ns, Ordering::Release);
        s.store(seq, Ordering::Release);
    }

    /// When block `seq` was handed over; `None` when it is no longer (or
    /// not yet) among the newest.
    pub fn published(&self, seq: u64) -> Option<u64> {
        let (s, t) = &self.slots[seq as usize % SLOTS];
        if seq == 0 || s.load(Ordering::Acquire) != seq {
            return None;
        }
        let ns = t.load(Ordering::Acquire);
        (s.load(Ordering::Acquire) == seq).then_some(ns)
    }
}

/// Bucket upper bounds, µs; the last bucket holds everything above.
const BOUNDS_US: [u64; 10] = [250, 500, 750, 1000, 1250, 1500, 2000, 2667, 4000, 8000];

/// Turnarounds of one interval.
#[derive(Default)]
pub struct Turnaround {
    buckets: [u64; BOUNDS_US.len() + 1],
    count: u64,
    over: u64,
    max_ns: u64,
}

/// One interval's turnarounds: how many, the bucket bounds holding the
/// median and the 99th percentile (µs; `u64::MAX` above the last bound),
/// the longest, and how many passed the block period.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub over: u64,
}

impl Turnaround {
    /// One turnaround of `ns` with blocks of `period_ns`.
    pub fn record(&mut self, ns: u64, period_ns: u64) {
        let us = ns / 1000;
        let i = BOUNDS_US
            .iter()
            .position(|&b| us <= b)
            .unwrap_or(BOUNDS_US.len());
        self.buckets[i] += 1;
        self.count += 1;
        if ns > period_ns {
            self.over += 1;
        }
        self.max_ns = self.max_ns.max(ns);
    }

    /// The interval so far, and a new one begins.
    pub fn take(&mut self) -> Summary {
        let s = Summary {
            count: self.count,
            p50_us: self.quantile(50),
            p99_us: self.quantile(99),
            max_us: self.max_ns / 1000,
            over: self.over,
        };
        *self = Turnaround::default();
        s
    }

    /// The upper bound of the bucket holding the `pct` percentile; 0 when
    /// nothing was recorded.
    fn quantile(&self, pct: u64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let want = (self.count * pct).div_ceil(100);
        let mut seen = 0;
        for (i, &n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= want {
                return BOUNDS_US.get(i).copied().unwrap_or(u64::MAX);
            }
        }
        u64::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_block_reads_back_its_time() {
        let s = Stamps::default();
        assert_eq!(s.published(1), None);
        s.publish(1, 100);
        s.publish(2, 200);
        assert_eq!(s.published(1), Some(100));
        assert_eq!(s.published(2), Some(200));
        // Seq 0 is never a block.
        assert_eq!(s.published(0), None);
    }

    #[test]
    fn a_block_older_than_the_ring_is_gone() {
        let s = Stamps::default();
        for seq in 1..=(SLOTS as u64 + 1) {
            s.publish(seq, seq * 10);
        }
        // Seq 1 shared its slot with seq 9.
        assert_eq!(s.published(1), None);
        assert_eq!(s.published(SLOTS as u64 + 1), Some((SLOTS as u64 + 1) * 10));
        assert_eq!(s.published(2), Some(20));
    }

    #[test]
    fn the_summary_holds_the_quantiles_the_max_and_the_late_ones() {
        let mut t = Turnaround::default();
        let period = 2_666_667;
        // 98 at 0.9 ms, one at 1.9 ms, one at 3.5 ms.
        for _ in 0..98 {
            t.record(900_000, period);
        }
        t.record(1_900_000, period);
        t.record(3_500_000, period);
        let s = t.take();
        assert_eq!(
            s,
            Summary {
                count: 100,
                p50_us: 1000,
                p99_us: 2000,
                max_us: 3500,
                over: 1,
            }
        );
        // A new interval.
        assert_eq!(
            t.take(),
            Summary {
                count: 0,
                p50_us: 0,
                p99_us: 0,
                max_us: 0,
                over: 0,
            }
        );
    }

    #[test]
    fn bucket_edges_and_the_open_top() {
        let mut t = Turnaround::default();
        // Exactly on a bound belongs to it; above the last bound is open.
        t.record(250_000, 1_000_000);
        assert_eq!(t.take().p50_us, 250);
        t.record(251_000, 1_000_000);
        assert_eq!(t.take().p50_us, 500);
        t.record(9_000_000, 1_000_000);
        let s = t.take();
        assert_eq!((s.p50_us, s.over, s.max_us), (u64::MAX, 1, 9000));
        // On the period exactly is not over it.
        t.record(1_000_000, 1_000_000);
        assert_eq!(t.take().over, 0);
    }
}
