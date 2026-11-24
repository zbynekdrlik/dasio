//! The loop check on program audio (dasio#207, 24.11.2026). A device that
//! plays from two ASIO half-buffers and is not served (its host's callback
//! stalls, or its own interrupt path does) plays the same samples again and
//! again. On 24.11.2026 master's outputs repeated 256 samples about 20
//! times (55 ms) right after a song jump of band Live. The zero/repeat
//! watch does not see that: it compares with one block before, a run may be
//! eight blocks at most and the channel must be loud. The splice detector
//! logs a train of steps, which only a pure tone lets one read as a loop.
//!
//! Per program channel, per sample: for each lag of 1, 2 and 4 host blocks
//! ([`LAGS`]), the run of samples exactly equal to the sample that lag
//! before. A run that ends after at least one lag less
//! [`TOLERANCE`](super::TOLERANCE) samples, in which at least one sample in
//! [`VARY`] differs from the one before it ([`varied`]: not silence, not one
//! held value, not a few stray steps in silence one lag apart), is a
//! **loop**: a [`Loop`] with the media position of its first repeated
//! sample, the lag, the run's length and its loudest sample. The lags are judged smallest first and one
//! sample reports once, so a loop of one block is not reported again at two
//! and four.
//!
//! Real 24-bit audio that moves from sample to sample never repeats sample
//! for sample, so there is no level floor: the log line carries the peak
//! for how loud the loop was. A bit-exact signal whose period divides a lag
//! repeats for as long as it plays: one event, when it stops. A loop of
//! which fewer than one sample in [`VARY`] changes (a ring that is mostly
//! digital silence or one held value) is not reported.
//!
//! The state is one fixed ring per channel, made once ([`Channel::new`]);
//! nothing allocates, locks or blocks (it runs on the pump path like the
//! other checks). The loops go into [`LoopEvents`]; the run loop turns them
//! into log lines ([`lines`]: channels that looped at the same sample with
//! the same lag are one line).

use super::{Ring, Slot, TOLERANCE};

/// The lags checked, in host blocks (the config's `block`).
pub const LAGS: [usize; 3] = [1, 2, 4];
/// A loop's run has at least one sample in this many that differs from the
/// sample before it. Silence with two stray 24-bit steps one lag apart
/// repeats sample for sample too, with two changes in the whole run.
pub const VARY: u64 = 16;

/// One loop on one program channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    /// The channel's place in the config's `program` list.
    pub channel: usize,
    /// The media position of the first repeated sample.
    pub at: u64,
    /// Samples between a sample and its copy.
    pub lag: u32,
    /// The repeated samples.
    pub len: u32,
    /// The loudest repeated sample (linear, 1.0 is full scale).
    pub peak: f64,
}

/// The loops: a slot is `[at, info, peak]`; `info` packs the channel (bits
/// 48-63), the lag (bits 32-47: four blocks of 8192 at most) and the length
/// (bits 0-31).
pub type LoopEvents = Ring<Loop, 3>;

impl Slot<3> for Loop {
    fn pack(&self) -> [u64; 3] {
        // The three fields never overlap: a sum, so no `|`/`^` twin.
        [
            self.at,
            ((self.channel as u64) << 48) + (u64::from(self.lag) << 32) + u64::from(self.len),
            self.peak.to_bits(),
        ]
    }

    fn unpack([at, info, peak]: [u64; 3]) -> Self {
        Loop {
            channel: (info >> 48) as usize,
            at,
            lag: u32::from((info >> 32) as u16),
            len: info as u32,
            peak: f64::from_bits(peak),
        }
    }
}

/// The lags, samples, for a host block of `block` samples.
pub fn lags(block: usize) -> [u64; 3] {
    LAGS.map(|k| (k * block) as u64)
}

/// Whether a run of `run` repeated samples at `lag` is a loop: at least one
/// lag less [`TOLERANCE`](super::TOLERANCE).
pub fn looped(run: u32, lag: u64) -> bool {
    u64::from(run) + TOLERANCE as u64 >= lag
}

/// Whether a run of `len` repeated samples, `changes` of them unlike the
/// sample before, moves enough to be audio: one in [`VARY`] at least.
pub fn varied(changes: u32, len: u32) -> bool {
    u64::from(changes) * VARY >= u64::from(len)
}

/// `len` samples at `rate` in ms, to a tenth.
pub fn ms(len: u32, rate: u32) -> f64 {
    (f64::from(len) * 10_000.0 / f64::from(rate.max(1))).round() / 10.0
}

/// One lag's run of repeated samples.
#[derive(Clone, Copy, Default)]
struct Run {
    len: u32,
    start: u64,
    /// Samples of the run unlike the sample before them.
    changes: u32,
    peak: f64,
}

/// One program channel's state; allocation-free after [`Channel::new`].
pub(super) struct Channel {
    /// The last samples: a power of two of them, at least the largest lag.
    ring: Box<[f64]>,
    mask: u64,
    /// Samples fed since the start.
    n: u64,
    /// The sample before (a loop's run must move, [`varied`]).
    last: f64,
    lags: [u64; 3],
    runs: [Run; 3],
}

impl Channel {
    /// For a host block of `block` samples.
    pub(super) fn new(block: usize) -> Self {
        let lags = lags(block);
        let size = (lags[2] as usize).next_power_of_two();
        Self {
            ring: vec![0.0; size].into_boxed_slice(),
            mask: size as u64 - 1,
            n: 0,
            last: 0.0,
            lags,
            runs: [Run::default(); 3],
        }
    }

    /// Back to the start (a discontinuity), allocating nothing: a run in
    /// progress is dropped, like every check's state. The fill is four
    /// blocks of samples (2 KiB at a block of 64).
    pub(super) fn reset(&mut self) {
        self.ring.fill(0.0);
        self.last = 0.0;
        self.runs = [Run::default(); 3];
    }

    /// Sample `x` at media position `at`; loops into `out` as channel `ch`.
    pub(super) fn feed(&mut self, x: f64, at: u64, out: &LoopEvents, ch: usize) {
        let i = self.n;
        // One sample reports once: the smallest lag that looped.
        let mut told = false;
        for (run, &lag) in self.runs.iter_mut().zip(&self.lags) {
            // The slot of `i - lag` is read before sample `i` is written, so
            // a ring of exactly the largest lag is enough.
            if x == self.ring[(i.wrapping_sub(lag) & self.mask) as usize] {
                // A change counts only between samples of the run.
                if run.len == 0 {
                    run.start = at;
                    run.changes = 0;
                    run.peak = 0.0;
                } else if x != self.last {
                    run.changes = run.changes.saturating_add(1);
                }
                run.len = run.len.saturating_add(1);
                run.peak = run.peak.max(x.abs());
            } else if run.len != 0 {
                if !told && varied(run.changes, run.len) && looped(run.len, lag) {
                    out.push(Loop {
                        channel: ch,
                        at: run.start,
                        lag: lag as u32,
                        len: run.len,
                        peak: run.peak,
                    });
                    told = true;
                }
                run.len = 0;
            }
        }
        self.ring[(i & self.mask) as usize] = x;
        self.last = x;
        self.n = i + 1;
    }
}

/// One log line: the loops of one moment over the program channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The earliest loop's first repeated sample.
    pub at: u64,
    pub lag: u32,
    /// The longest of the line's loops, samples.
    pub len: u32,
    /// The loudest repeated sample of the line's loops.
    pub peak: f64,
    /// The channels (places in the config's `program`), ascending.
    pub channels: Vec<usize>,
}

/// `loops` (in push order) as lines: loops of one lag that start within one
/// sample of the line's first are one line.
pub fn lines(loops: &[Loop]) -> Vec<Line> {
    let mut sorted = loops.to_vec();
    sorted.sort_by_key(|l| (l.at, l.channel));
    let mut out: Vec<Line> = Vec::new();
    for l in sorted {
        // The newest line of this lag: a loop of another lag at the same
        // sample may lie between two channels of one.
        if let Some(line) = out
            .iter_mut()
            .rev()
            .find(|line| line.lag == l.lag && l.at - line.at <= 1)
        {
            line.channels.push(l.channel);
            line.channels.sort_unstable();
            line.channels.dedup();
            line.len = line.len.max(l.len);
            line.peak = line.peak.max(l.peak);
            continue;
        }
        out.push(Line {
            at: l.at,
            lag: l.lag,
            len: l.len,
            peak: l.peak,
            channels: vec![l.channel],
        });
    }
    out
}

#[cfg(test)]
mod tests;
