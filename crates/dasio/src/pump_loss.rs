//! The pump's lost blocks by cause (dasio#114): which of the blocks the
//! device's pump skipped the environment caused (the pump woke late) and
//! which the device did. Pure, so tested everywhere; the device's 100 ms
//! loop feeds it and publishes the totals in the shared header, where
//! `dasio probe` reads them.
//!
//! The pump skips blocks when it re-bootstraps (dstack's `Due::Rebootstrap`).
//! A re-bootstrap for `Cause::Late` means it woke more than its margin W
//! behind a block's due time; dstack records that wake (`WakeLateness`, with
//! `rebootstrap` set and how far behind it was) BEFORE it counts the
//! re-bootstrap, so the counts read first never name a wake the ring does
//! not hold yet. A pump woken late by the runner (its vCPU taken, CI
//! 43779634868) loses only such blocks; a pump behind on its own (a block
//! longer than its period, an overrun) is the device's.
//!
//! A look's new blocks are the environment's only when, since the look
//! before: at least one re-bootstrap for lateness and none of another
//! cause, no block overran in this look's window or the one before (a long
//! block of the device's own puts the pump behind for its next wake), every
//! late re-bootstrap counted so far has its own late wake in the ring
//! measured at or past W, and the ring's new wakes were all read. Anything
//! else is the device's. (dstack flags a wake late only past W, so the
//! margin check restates its rule: the gate is that each late
//! re-bootstrap's wake was read.)
//!
//! dstack counts a re-bootstrap's cause BEFORE it adds the skipped blocks
//! to `missed` (Release), and its `PumpView` loads `missed` first (Acquire)
//! (dstack #332): a look that sees skipped blocks sees their cause too, so
//! a look's blocks count in that look. A look can still fall between the
//! two and see a cause with none of its blocks: such a look judges
//! nothing, and the next look judges both windows together (one look at
//! most, as a re-bootstrap may skip no block).
//!
//! A runner stall that lands inside a block's processing reads as an
//! overrun: the device's, as the pump cannot tell its wall time from its
//! CPU time there.

/// The pump's cumulative counts at one look (dstack's `PumpView`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    /// Blocks the re-bootstraps skipped.
    pub missed: u64,
    /// Blocks longer than one period.
    pub overruns: u64,
    /// Re-bootstraps of any cause.
    pub rebootstraps: u64,
    /// Re-bootstraps for lateness (`Cause::Late`).
    pub rebootstraps_late: u64,
}

/// One late wake from the pump's ring (dstack's `LateWake`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wake {
    /// How far behind its due time the pump woke, whole µs.
    pub behind_us: u64,
    /// The pump re-bootstrapped at this wake.
    pub rebootstrap: bool,
}

/// The pump's late wakes as read at one look: `end`, the index past the
/// newest (dstack's `late_wakes`), and the newest of them, oldest first,
/// ending at `end` with none left out (a torn read is no `Wakes`).
#[derive(Debug, Clone, Copy)]
pub struct Wakes<'a> {
    pub end: u64,
    pub recent: &'a [Wake],
}

impl<'a> Wakes<'a> {
    /// The ring's read when it is whole: `end` the same before (`end`) and
    /// after (`end_after`) the read, and `recent` holding all of the last
    /// `kept` (the ring's size); `None` otherwise.
    pub fn whole(end: u64, recent: &'a [Wake], end_after: u64, kept: u64) -> Option<Self> {
        (end == end_after && recent.len() as u64 == end.min(kept)).then_some(Wakes { end, recent })
    }
}

/// The pump's lost time since the tracker's first look, µs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lost {
    /// Blocks lost to a late wake (the environment).
    pub late_us: u64,
    /// Every other lost block (the device).
    pub device_us: u64,
}

/// The device loop's memory between looks.
#[derive(Debug)]
pub struct Tracker {
    block_ns: u64,
    margin_us: u64,
    last: Option<Counts>,
    /// The ring's index past the last wake read.
    seen_end: u64,
    /// Late re-bootstraps whose wake was read and measured at or past W,
    /// settled up to the count after every read that moved `seen_end`.
    measured: u64,
    overran_before: bool,
    /// The last look judged nothing (a re-bootstrap counted, none of its
    /// blocks yet): this one judges both windows, and may not defer again.
    deferred: bool,
    late_ns: u64,
    device_ns: u64,
}

impl Tracker {
    /// A tracker for a pump of `block` frames and a margin of `margin`
    /// frames at `rate` Hz.
    pub fn new(block: u64, margin: u64, rate: u32) -> Self {
        let rate = u64::from(rate.max(1));
        Tracker {
            block_ns: block * 1_000_000_000 / rate,
            margin_us: margin * 1_000_000 / rate,
            last: None,
            seen_end: 0,
            measured: 0,
            overran_before: false,
            deferred: false,
            late_ns: 0,
            device_ns: 0,
        }
    }

    /// One look: the pump's counts `now` (read first) and its late wakes
    /// (`None` for a torn read). The first look, one whose counts went
    /// down and one whose ring went back (a new engine) only learn them.
    /// Returns the totals, this look's blocks in them (a look that saw a
    /// re-bootstrap but none of its blocks judges nothing, see `deferred`).
    pub fn take(&mut self, now: Counts, wakes: Option<Wakes>) -> Lost {
        let before = self.last.replace(now);
        let ring_back = wakes.is_some_and(|w| w.end < self.seen_end);
        let Some(before) = before.filter(|b| {
            !ring_back
                && now.missed >= b.missed
                && now.overruns >= b.overruns
                && now.rebootstraps >= b.rebootstraps
                && now.rebootstraps_late >= b.rebootstraps_late
        }) else {
            self.seen_end = wakes.map_or(0, |w| w.end);
            self.measured = now.rebootstraps_late;
            self.overran_before = false;
            self.deferred = false;
            return self.lost();
        };
        // A re-bootstrap counted and none of its blocks yet (dstack adds
        // them right after the cause): the next look judges this window
        // with its own, from the same baseline.
        if !self.deferred && now.rebootstraps > before.rebootstraps && now.missed == before.missed {
            self.deferred = true;
            self.last = Some(before);
            return self.lost();
        }
        self.deferred = false;
        let mut read = false;
        if let Some(w) = wakes {
            let new = w.end - self.seen_end;
            let len = w.recent.len() as u64;
            if new <= len {
                let fresh = &w.recent[(len - new) as usize..];
                self.measured += fresh
                    .iter()
                    .filter(|x| x.rebootstrap && x.behind_us >= self.margin_us)
                    .count() as u64;
                read = true;
            }
            self.seen_end = w.end;
        }
        let overran = now.overruns > before.overruns;
        let only_late = now.rebootstraps - before.rebootstraps
            == now.rebootstraps_late - before.rebootstraps_late;
        let late = read
            && only_late
            && now.rebootstraps_late > before.rebootstraps_late
            && !overran
            && !self.overran_before
            && self.measured >= now.rebootstraps_late;
        let ns = (now.missed - before.missed) * self.block_ns;
        if late {
            self.late_ns += ns;
        } else {
            self.device_ns += ns;
        }
        self.overran_before = overran;
        // Every wake up to `seen_end` was judged: a late re-bootstrap
        // without its measured wake (a ring read past, a wake short of W)
        // was the device's in its look; the next looks start even. A torn
        // read moved nothing: its wakes are read next time.
        if wakes.is_some() {
            self.measured = self.measured.max(now.rebootstraps_late);
        }
        self.lost()
    }

    fn lost(&self) -> Lost {
        Lost {
            late_us: self.late_ns / 1_000,
            device_us: self.device_ns / 1_000,
        }
    }
}

/// `us` as whole blocks of `block` frames at `rate` Hz, rounded down: the
/// switches a host at that block lost in it.
pub fn blocks_of(us: u64, rate: u32, block: u32) -> u64 {
    if block == 0 {
        return 0;
    }
    us * u64::from(rate) / 1_000_000 / u64::from(block)
}

#[cfg(test)]
mod tests;
