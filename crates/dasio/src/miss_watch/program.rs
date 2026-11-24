//! The splice detector on program audio (dasio#207, owner 24.11.2026: the
//! breaks that are heard are in the music and the sermon of a real service,
//! where no test tone can play). When the host misses a callback the output
//! goes on with a block's worth of signal skipped or repeated: neither an
//! exact zero run nor an exact repeat, but a step between two valid
//! waveforms. Per program channel, per sample:
//!
//! - `h[n] = x[n] - 4 x[n-1] + 6 x[n-2] - 4 x[n-3] + x[n-4]`, the fourth
//!   difference ([`fourth`]), and `e[n] = |h[n]|`. A step of `J` passes as
//!   `J x [1, -3, 3, -1]` (peak `3 J`); band-limited audio is cut hard
//!   (x0.17 at 10 kHz, 96 kHz rate), a splice has energy everywhere.
//! - an **event** is a local peak of `e` (the largest within +-[`NEAR`]
//!   samples, the first of equal ones: a step has two) that stands [`K`]
//!   times above the mean of `e` over the [`WIN`] samples ending [`GAP`]
//!   samples before it AND over the [`WIN`] samples starting [`GAP`] samples
//!   after it, and above [`FLOOR`] ([`jump`]). A musical onset keeps `e`
//!   raised afterwards, a splice is isolated.
//! - the decision about a peak at `p` is made when sample `p + 263` arrives
//!   ([`DELAY`]): the state is fixed-size rings and sums per channel, made
//!   once ([`Channel::new`]); nothing allocates, locks or blocks (it runs
//!   on the pump path like the tone check).
//! - after an event the channel holds off [`HOLD`] samples ([`held`]), so
//!   one splice is one event.
//!
//! The events go into [`ProgramEvents`] (a fixed ring, like the tone's),
//! each block's figures into [`Stats`]. The run loop turns the events into
//! log lines ([`Tracker::lines`]: channels that jumped at the same sample
//! are one line, with the distance to the channel's previous event and its
//! remainder in host blocks: the host's misses are whole blocks apart, a
//! music false hit is not) and every 10 s logs each channel's figures
//! ([`Programs::take`]).
//!
//! The 24-bit RX makes `e` a whole number of 2^-23 steps, so the sliding
//! sums are exact.

use std::sync::atomic::{AtomicU64, Ordering};

use super::looped::LoopEvents;
use super::{Ring, Slot};

/// Samples of the two windows whose means a peak is judged against.
pub const WIN: usize = 256;
/// Samples between a peak and each window.
pub const GAP: usize = 8;
/// A peak is the largest of `e` within this many samples either way.
pub const NEAR: usize = 4;
/// Samples after an event in which the channel reports no other.
pub const HOLD: u64 = 512;
/// How many times a peak must stand above the mean of `e` of each window.
/// PROVISIONAL until the FOH calibration run (the tone and this detector
/// on the same breaks, dasio#207): set from that run, then not touched.
pub const K: f64 = 16.0;
/// The absolute floor of a peak of `e`: 1024 steps of 24-bit full scale
/// (-78 dBFS).
pub const FLOOR: f64 = 1024.0 / 8_388_608.0;
/// One 24-bit step: the least a window's mean counts as, so a dB figure
/// over silence stays finite.
pub const QUANT: f64 = 1.0 / 8_388_608.0;
/// Samples from a peak to the last sample of its later window.
pub const DELAY: usize = GAP + WIN - 1;

const RING: usize = 1024;
const MASK: u64 = RING as u64 - 1;
/// The first samples' `h` is built on the history before the start; by the
/// first decision it has left both windows. The first sample judged is
/// `p = WARM - DELAY`.
const WARM: u64 = 4 + 2 * DELAY as u64;
/// A sample is judged when a sample not exactly zero lies within the
/// samples its decision spans (`p - 267` to `p + 263`).
const SPAN: u64 = WARM + 1;
/// Fixed-point scale of the summed `e` (2^32 per unit).
const FIXED: f64 = 4_294_967_296.0;

/// One event on one program channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgramJump {
    /// The channel's place in the config's `program` list.
    pub channel: usize,
    /// The media position of the peak.
    pub at: u64,
    /// The peak over the larger of the two window means (at least
    /// [`QUANT`]).
    pub ratio: f64,
    /// The peak of `e` itself, in 24-bit steps.
    pub steps: u64,
}

/// The events: a slot is `[at, channel, ratio, steps]`.
pub type ProgramEvents = Ring<ProgramJump, 4>;

impl Slot<4> for ProgramJump {
    fn pack(&self) -> [u64; 4] {
        [
            self.at,
            self.channel as u64,
            self.ratio.to_bits(),
            self.steps,
        ]
    }

    fn unpack([at, channel, ratio, steps]: [u64; 4]) -> Self {
        ProgramJump {
            channel: channel as usize,
            at,
            ratio: f64::from_bits(ratio),
            steps,
        }
    }
}

/// One channel's figures since the run loop last took them. Non-negative
/// f64s order like their bits, so the worst is kept with an integer max.
#[derive(Default)]
pub struct Stats {
    checked: AtomicU64,
    jumps: AtomicU64,
    worst: AtomicU64,
    esum: AtomicU64,
}

/// A program channel's figures over a stretch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Samples judged (not within digital silence).
    pub checked: u64,
    pub jumps: u64,
    /// The largest peak-over-window-mean ratio of any local peak above
    /// [`FLOOR`], event or not (0 with none): how near to [`K`] a clean
    /// stretch came.
    pub worst: f64,
    /// The sum of `e` over the judged samples, 2^-32 units.
    pub esum: u64,
}

impl Stats {
    /// The figures since the last take; the next stretch starts empty.
    pub fn take(&self) -> Summary {
        Summary {
            checked: self.checked.swap(0, Ordering::Relaxed),
            jumps: self.jumps.swap(0, Ordering::Relaxed),
            worst: f64::from_bits(self.worst.swap(0, Ordering::Relaxed)),
            esum: self.esum.swap(0, Ordering::Relaxed),
        }
    }
}

/// The program side's output for the run: its events, the loop check's
/// ([`super::looped`]) and each channel's figures.
pub struct Programs {
    pub events: ProgramEvents,
    pub loops: LoopEvents,
    stats: Box<[Stats]>,
}

impl Programs {
    /// For `channels` program channels.
    pub fn new(channels: usize) -> Self {
        Self {
            events: ProgramEvents::default(),
            loops: LoopEvents::default(),
            stats: (0..channels).map(|_| Stats::default()).collect(),
        }
    }

    /// The figures of program channel `channel` since the last take.
    pub fn take(&self, channel: usize) -> Summary {
        self.stats[channel].take()
    }

    pub(super) fn stats(&self, channel: usize) -> &Stats {
        &self.stats[channel]
    }
}

/// The fourth difference at `x`, given the four samples before it, newest
/// first.
pub fn fourth(x: f64, before: &[f64; 4]) -> f64 {
    x - 4.0 * before[0] + 6.0 * before[1] - 4.0 * before[2] + before[3]
}

/// Whether a peak of `e` is an event: above [`FLOOR`] and at least [`K`]
/// times each window's mean.
pub fn jump(peak: f64, before: f64, after: f64) -> bool {
    above_floor(peak) && peak >= K * before && peak >= K * after
}

/// Whether a peak of `e` is above the absolute floor, [`FLOOR`].
pub fn above_floor(peak: f64) -> bool {
    peak > FLOOR
}

/// The peak over the larger of the two window means, at least [`QUANT`].
pub fn ratio(peak: f64, before: f64, after: f64) -> f64 {
    peak / before.max(after).max(QUANT)
}

/// A peak of `e` in 24-bit steps (`e` is a whole number of them).
pub fn steps(peak: f64) -> u64 {
    (peak / QUANT).round() as u64
}

/// A peak of `steps` 24-bit steps in dBFS, to a tenth (at least one step,
/// so it stays finite).
pub fn peak_dbfs(steps: u64) -> f64 {
    (super::tone::db(steps.max(1) as f64 * QUANT) * 10.0).round() / 10.0
}

/// An event's place on the block grid: `at` modulo `block`.
pub fn grid(at: u64, block: usize) -> u64 {
    at % block as u64
}

/// A window's mean of `e` from its running sum (a sum that rounding left
/// under zero is zero).
pub fn mean(sum: f64) -> f64 {
    sum.max(0.0) / WIN as f64
}

/// Whether the peak at `p` is held off by the event at `last`.
pub fn held(p: u64, last: Option<u64>) -> bool {
    last.is_some_and(|l| p - l < HOLD)
}

/// Whether a stretch of `quiet` samples exactly zero up to now leaves a
/// decision something to judge.
pub fn judged(quiet: u64) -> bool {
    quiet < SPAN
}

/// Whether `e[p]` is the largest within [`NEAR`] samples either way, the
/// first of equal ones; `e` is the ring of the last [`RING`] values of `e`.
pub fn local_peak(e: &[f64], p: u64) -> bool {
    let v = e[(p & MASK) as usize];
    (1..=NEAR as u64)
        .all(|k| e[(p.wrapping_sub(k) & MASK) as usize] < v && e[((p + k) & MASK) as usize] <= v)
}

/// The mean of `e` over a stretch, dBFS, from [`Summary`]'s figures (at
/// least -240).
pub fn floor_dbfs(s: &Summary) -> f64 {
    let mean = s.esum as f64 / FIXED / s.checked.max(1) as f64;
    super::tone::db(mean.max(1e-12))
}

/// One program channel's state; allocation-free after [`Channel::new`].
pub(super) struct Channel {
    /// The device's RX index.
    pub(super) rx: usize,
    /// The four samples before the next, newest first.
    before: [f64; 4],
    /// Samples fed since the start.
    n: u64,
    /// The last [`RING`] values of `e`.
    e: Box<[f64]>,
    /// Sums of `e` over the later window (the newest [`WIN`]) and over the
    /// earlier one.
    later: f64,
    earlier: f64,
    /// Samples exactly zero in a row up to now.
    quiet: u64,
    /// The last event's peak position, in samples since the start.
    last: Option<u64>,
    /// This block's figures for [`Stats`].
    checked: u64,
    jumps: u64,
    worst: f64,
    esum: u64,
}

impl Channel {
    pub(super) fn new(rx: usize) -> Self {
        Self {
            rx,
            before: [0.0; 4],
            n: 0,
            e: vec![0.0; RING].into_boxed_slice(),
            later: 0.0,
            earlier: 0.0,
            quiet: 0,
            last: None,
            checked: 0,
            jumps: 0,
            worst: 0.0,
            esum: 0,
        }
    }

    /// Back to the start (a discontinuity), allocating nothing.
    pub(super) fn reset(&mut self) {
        self.before = [0.0; 4];
        self.n = 0;
        self.e.fill(0.0);
        self.later = 0.0;
        self.earlier = 0.0;
        self.quiet = 0;
        self.last = None;
    }

    fn at(&self, i: u64) -> f64 {
        self.e[(i & MASK) as usize]
    }

    /// Sample `x` at media position `at`; events into `out` as channel `ch`.
    pub(super) fn feed(&mut self, x: f64, at: u64, out: &ProgramEvents, ch: usize) {
        let i = self.n;
        let e = fourth(x, &self.before).abs();
        // Slots not yet written read as zero: the windows start empty.
        self.later += e - self.at(i.wrapping_sub(WIN as u64));
        self.earlier += self.at(i.wrapping_sub((DELAY + GAP) as u64))
            - self.at(i.wrapping_sub((DELAY + GAP + WIN) as u64));
        self.e[(i & MASK) as usize] = e;
        self.quiet = if x == 0.0 { self.quiet + 1 } else { 0 };
        if i >= WARM {
            self.judge(i - DELAY as u64, at.saturating_sub(DELAY as u64), out, ch);
        }
        self.before = [x, self.before[0], self.before[1], self.before[2]];
        self.n = i + 1;
    }

    /// The decision about the peak at sample `p` (media position `at`).
    fn judge(&mut self, p: u64, at: u64, out: &ProgramEvents, ch: usize) {
        let peak = self.at(p);
        if judged(self.quiet) {
            self.checked += 1;
            self.esum += (peak * FIXED) as u64;
        }
        if !above_floor(peak) || !local_peak(&self.e, p) {
            return;
        }
        let (before, after) = (mean(self.earlier), mean(self.later));
        let r = ratio(peak, before, after);
        self.worst = self.worst.max(r);
        if jump(peak, before, after) && !held(p, self.last) {
            self.last = Some(p);
            self.jumps += 1;
            out.push(ProgramJump {
                channel: ch,
                at,
                ratio: r,
                steps: steps(peak),
            });
        }
    }

    /// This block's figures into `s`; the next block's start empty.
    pub(super) fn publish(&mut self, s: &Stats) {
        s.checked.fetch_add(self.checked, Ordering::Relaxed);
        s.jumps.fetch_add(self.jumps, Ordering::Relaxed);
        s.worst.fetch_max(self.worst.to_bits(), Ordering::Relaxed);
        s.esum.fetch_add(self.esum, Ordering::Relaxed);
        self.checked = 0;
        self.jumps = 0;
        self.worst = 0.0;
        self.esum = 0;
    }
}

/// One log line: the events of one moment over the program channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The earliest event's peak position.
    pub at: u64,
    /// The largest ratio of the line's events.
    pub ratio: f64,
    /// The largest peak of the line's events, 24-bit steps.
    pub steps: u64,
    /// The earliest event's place on the block grid, `at % block`.
    pub grid: u64,
    /// The channels (places in the config's `program`), ascending.
    pub channels: Vec<usize>,
    /// Samples since the previous event of the line's first event's channel,
    /// and that modulo the host block; none for that channel's first event.
    pub since_prev: Option<u64>,
    pub rem: Option<u64>,
}

/// The run loop's memory of each channel's last event.
#[derive(Default)]
pub struct Tracker {
    last: Vec<Option<u64>>,
}

impl Tracker {
    /// `jumps` (in push order) as lines: events within one sample of the
    /// line's first are one line. `block` is the host's.
    pub fn lines(&mut self, jumps: &[ProgramJump], block: usize) -> Vec<Line> {
        let mut sorted = jumps.to_vec();
        sorted.sort_by_key(|j| (j.at, j.channel));
        let mut out: Vec<Line> = Vec::new();
        for j in sorted {
            if self.last.len() <= j.channel {
                self.last.resize(j.channel + 1, None);
            }
            match out.last_mut() {
                Some(l) if j.at - l.at <= 1 => {
                    if !l.channels.contains(&j.channel) {
                        l.channels.push(j.channel);
                        l.channels.sort_unstable();
                    }
                    l.ratio = l.ratio.max(j.ratio);
                    l.steps = l.steps.max(j.steps);
                }
                _ => {
                    let since = self.last[j.channel].and_then(|p| j.at.checked_sub(p));
                    out.push(Line {
                        at: j.at,
                        ratio: j.ratio,
                        steps: j.steps,
                        grid: grid(j.at, block),
                        channels: vec![j.channel],
                        since_prev: since,
                        rem: since.map(|s| s % block as u64),
                    });
                }
            }
            self.last[j.channel] = Some(j.at);
        }
        out
    }
}

#[cfg(test)]
mod tests;
