//! The DAW's turnaround: from a block's hand-over (the bridge publishes it)
//! until the driver returns its outputs (the header's ack), in ns. A block
//! whose turnaround passes one block period is counted late by the bridge
//! (its outputs are sent as silence). Measured to tell a slow DAW from a
//! late wake-up on the FOH PC (dasio#1). A driver that sends, with each
//! block's ack, when the DAW's callback for it started
//! (`shm::Header::ack_block`) splits it in two: the wake-up (hand-over
//! until the callback starts: Windows waking the driver's thread) and the
//! processing (callback start until the outputs return: the DAW's own
//! work), dasio#22.

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

/// One interval's times in buckets, with the longest.
#[derive(Default)]
struct Histogram {
    buckets: [u64; BOUNDS_US.len() + 1],
    count: u64,
    max_ns: u64,
}

impl Histogram {
    fn record(&mut self, ns: u64) {
        let us = ns / 1000;
        let i = BOUNDS_US
            .iter()
            .position(|&b| us <= b)
            .unwrap_or(BOUNDS_US.len());
        self.buckets[i] += 1;
        self.count += 1;
        self.max_ns = self.max_ns.max(ns);
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

/// Turnarounds of one interval, and their split where the driver gave one.
#[derive(Default)]
pub struct Turnaround {
    total: Histogram,
    over: u64,
    wake: Histogram,
    process: Histogram,
}

/// One interval's turnarounds: how many, the bucket bounds holding the
/// median and the 99th percentile (µs; `u64::MAX` above the last bound),
/// the longest, and how many passed the block period. `split` of them came
/// with the driver's callback start: their wake-up and processing parts.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub over: u64,
    pub split: u64,
    pub wake_p99_us: u64,
    pub wake_max_us: u64,
    pub process_p99_us: u64,
    pub process_max_us: u64,
}

/// A block's turnaround from its hand-over at `published` until its ack
/// seen at `now`, split at the driver's callback start `entered` (all ns
/// on one clock): `(total, Some((wake, process)))`. No split without a
/// start, or with one outside the turnaround (another clock, a stale
/// value): the total still counts.
pub fn split(published: u64, entered: Option<u64>, now: u64) -> (u64, Option<(u64, u64)>) {
    let total = now.saturating_sub(published);
    let parts = entered
        .filter(|e| (published..=now).contains(e))
        .map(|e| (e - published, now - e));
    (total, parts)
}

impl Turnaround {
    /// One turnaround of `ns` with blocks of `period_ns`, and its split
    /// into wake-up and processing when known.
    pub fn record(&mut self, ns: u64, period_ns: u64, parts: Option<(u64, u64)>) {
        self.total.record(ns);
        if ns > period_ns {
            self.over += 1;
        }
        if let Some((wake, process)) = parts {
            self.wake.record(wake);
            self.process.record(process);
        }
    }

    /// The interval so far, and a new one begins.
    pub fn take(&mut self) -> Summary {
        let s = Summary {
            count: self.total.count,
            p50_us: self.total.quantile(50),
            p99_us: self.total.quantile(99),
            max_us: self.total.max_ns / 1000,
            over: self.over,
            split: self.wake.count,
            wake_p99_us: self.wake.quantile(99),
            wake_max_us: self.wake.max_ns / 1000,
            process_p99_us: self.process.quantile(99),
            process_max_us: self.process.max_ns / 1000,
        };
        *self = Turnaround::default();
        s
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
            t.record(900_000, period, None);
        }
        t.record(1_900_000, period, None);
        t.record(3_500_000, period, None);
        let s = t.take();
        assert_eq!(
            s,
            Summary {
                count: 100,
                p50_us: 1000,
                p99_us: 2000,
                max_us: 3500,
                over: 1,
                split: 0,
                wake_p99_us: 0,
                wake_max_us: 0,
                process_p99_us: 0,
                process_max_us: 0,
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
                split: 0,
                wake_p99_us: 0,
                wake_max_us: 0,
                process_p99_us: 0,
                process_max_us: 0,
            }
        );
    }

    #[test]
    fn bucket_edges_and_the_open_top() {
        let mut t = Turnaround::default();
        // Exactly on a bound belongs to it; above the last bound is open.
        t.record(250_000, 1_000_000, None);
        assert_eq!(t.take().p50_us, 250);
        t.record(251_000, 1_000_000, None);
        assert_eq!(t.take().p50_us, 500);
        t.record(9_000_000, 1_000_000, None);
        let s = t.take();
        assert_eq!((s.p50_us, s.over, s.max_us), (u64::MAX, 1, 9000));
        // On the period exactly is not over it.
        t.record(1_000_000, 1_000_000, None);
        assert_eq!(t.take().over, 0);
    }

    #[test]
    fn a_turnaround_splits_at_the_callback_start() {
        // Handed over at 1000, callback entered at 1300, ack seen at 2000.
        assert_eq!(split(1_000, Some(1_300), 2_000), (1_000, Some((300, 700))));
        // An older driver gives no start: the total alone.
        assert_eq!(split(1_000, None, 2_000), (1_000, None));
        // A start outside the turnaround (another clock, a stale value)
        // is no split; the total still counts.
        assert_eq!(split(1_000, Some(900), 2_000), (1_000, None));
        assert_eq!(split(1_000, Some(2_100), 2_000), (1_000, None));
        // On the edges it is a split.
        assert_eq!(split(1_000, Some(1_000), 2_000), (1_000, Some((0, 1_000))));
        assert_eq!(split(1_000, Some(2_000), 2_000), (1_000, Some((1_000, 0))));
        // A clock that went back is a zero total, never a wrap.
        assert_eq!(split(2_000, None, 1_000), (0, None));
    }

    #[test]
    fn the_summary_holds_each_part_s_p99_and_max() {
        let mut t = Turnaround::default();
        let period = 2_666_667;
        // 99 blocks: 100 µs wake-up, 1 ms processing.
        for _ in 0..99 {
            t.record(1_100_000, period, Some((100_000, 1_000_000)));
        }
        // One stall that is Windows waking us late: 6 ms, then 0.9 ms.
        t.record(6_900_000, period, Some((6_000_000, 900_000)));
        // One block with no split (an older driver's).
        t.record(800_000, period, None);
        let s = t.take();
        assert_eq!((s.count, s.split, s.over), (101, 100, 1));
        assert_eq!((s.wake_p99_us, s.wake_max_us), (250, 6_000));
        assert_eq!((s.process_p99_us, s.process_max_us), (1_000, 1_000));
        assert_eq!(s.max_us, 6_900);
        // A new interval forgets the parts too.
        let s = t.take();
        assert_eq!((s.split, s.wake_max_us, s.process_max_us), (0, 0, 0));
    }

    #[test]
    fn a_long_callback_shows_in_processing_not_wake_up() {
        let mut t = Turnaround::default();
        t.record(17_200_000, 2_666_667, Some((50_000, 17_150_000)));
        let s = t.take();
        assert_eq!((s.wake_max_us, s.process_max_us), (50, 17_150));
        assert_eq!(s.process_p99_us, u64::MAX);
        assert_eq!(s.wake_p99_us, 250);
    }
}
