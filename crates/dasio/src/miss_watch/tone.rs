//! The test-tone check (dasio#207, owner 21.11.2026: a continuous tone, so
//! that sporadic damage between the words of a marker signal is seen too).
//! A pure sine obeys `x[n+1] + x[n-1] = c x[n]` with `c = 2 cos w`, so on
//! every sample the residual `r = x[n+1] + x[n-1] - c x[n]` of a clean tone
//! is its quantisation only, and any damage shows: a click of size `d` gives
//! `|r| ~ d`; a zero run, a slipped or repeated sample, a phase jump give
//! `|r| ~ A w`. Per tone channel:
//!
//! - **acquire**: `c` by least squares over [`ACQUIRE`] samples, with the
//!   tone's amplitude `A` and the fit's own residual. The tone settles when
//!   its frequency lies from [`F_MIN`] Hz to [`F_MAX`] of the rate, `A` is
//!   above -80 dBFS ([`present`]) and the fit's RMS residual is at most half
//!   the break threshold ([`settles`]). Nothing is reported meanwhile.
//! - **settled**: a sample whose residual is beyond the threshold `A` x
//!   [`REL`] (-60 dB; at least [`QFLOOR`], 24-bit quantisation's bound)
//!   starts a click. Each window of a tone period (at least [`LEVEL_MIN`]
//!   samples) measures `A` again (`x[n]^2 + ((x[n+1] - x[n-1]) / 2 sin w)^2`,
//!   exact for a sine) and compares it with the running amplitude (time
//!   constant [`RUN_S`]): a step beyond 0.1 dB ([`level_break`]) is a level
//!   break; a tone that faded to -80 dBFS is acquired again, unreported.
//! - **broken**: the click lasts until [`RESUME`] clean residuals in a row
//!   with the tone there (a sample beyond the threshold); its length is the
//!   damaged run (a zero run's own length; a slip or a phase jump: 1), its
//!   size the worst residual over `A`. One that does not end within a second
//!   (the tone stopped) ends there.
//!
//! After any break the channel acquires again, so one glitch is one break:
//! a repeated block's two edges, a gap's start and end. The pump thread runs
//! it through [`super::Watch`]; the breaks go into [`ToneEvents`], each
//! block's figures into [`Stats`]. The run loop logs each break and every
//! 10 s each channel's figures ([`Tones::take`]).

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Ring, Slot};

/// A sample is broken when its residual is beyond this share of the tone's
/// amplitude: -60 dB.
pub const REL: f64 = 1e-3;
/// The least break threshold, four 24-bit steps (-126 dBFS): a clean tone
/// quantised to 24 bits has residuals of at most two. Under -66 dBFS it is
/// this, not [`REL`], that holds.
pub const QFLOOR: f64 = 4.0 / 8_388_608.0;
/// The gate: a tone at or under this amplitude (-80 dBFS) is not checked.
pub const AMP_MIN: f64 = 1e-4;
/// A level break: a window's amplitude more than this ratio (0.1 dB) above
/// the running amplitude, or under it.
pub const LEVEL_STEP: f64 = 1.011_579_454_259_898_6;
/// The residuals one fit of the tone takes.
pub const ACQUIRE: usize = 2048;
/// Clean residuals in a row, with the tone there, that end a click.
pub const RESUME: usize = 16;
/// The shortest level window, samples; a longer tone period is the window.
pub const LEVEL_MIN: usize = 256;
/// The running amplitude's time constant, s.
pub const RUN_S: f64 = 0.5;
/// The lowest tone checked, Hz: under it 24-bit quantisation leaves the fit
/// too coarse for the 0.1 dB level check at -80 dBFS.
pub const F_MIN: f64 = 40.0;
/// The highest tone checked, as a share of the rate.
pub const F_MAX: f64 = 0.45;

/// A tone break's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneKind {
    /// Samples off the sine: a click, a gap, a slip, a repeat, a jump.
    Click,
    /// A step in the tone's level.
    Level,
}

impl ToneKind {
    pub fn name(self) -> &'static str {
        match self {
            ToneKind::Click => "click",
            ToneKind::Level => "level",
        }
    }
}

/// One break of a watched tone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneBreak {
    /// The channel's place in the config's `tone` list.
    pub channel: usize,
    pub kind: ToneKind,
    /// The media position of a click's first damaged sample, or of a level
    /// window's first sample.
    pub at: u64,
    /// A click's damaged run (the rate's samples when it did not end within
    /// a second), or a level window's length, samples.
    pub len: u32,
    /// A click's worst residual over the tone's amplitude; a level break's
    /// window amplitude over the running amplitude.
    pub rel: f64,
}

/// The tone breaks: a slot is `[at, info, rel]`; `info` packs the channel
/// (bits 40-55), the kind (bit 32) and the length (bits 0-31).
pub type ToneEvents = Ring<ToneBreak, 3>;

impl Slot<3> for ToneBreak {
    fn pack(&self) -> [u64; 3] {
        let kind = match self.kind {
            ToneKind::Click => 0,
            ToneKind::Level => 1,
        };
        // The three fields never overlap: a sum, so no `|`/`^` twin.
        [
            self.at,
            ((self.channel as u64 & 0xFFFF) << 40) + (kind << 32) + u64::from(self.len),
            self.rel.to_bits(),
        ]
    }

    fn unpack([at, info, rel]: [u64; 3]) -> Self {
        ToneBreak {
            channel: ((info >> 40) & 0xFFFF) as usize,
            kind: if (info >> 32) & 1 == 1 {
                ToneKind::Level
            } else {
                ToneKind::Click
            },
            at,
            len: (info & 0xFFFF_FFFF) as u32,
            rel: f64::from_bits(rel),
        }
    }
}

/// One tone channel's figures since the run loop last took them: the pump
/// adds each block's, the loop takes them ([`Stats::take`]). Non-negative
/// f64s order like their bits, so the worst, least and largest are kept
/// with integer max and min.
pub struct Stats {
    checked: AtomicU64,
    breaks: AtomicU64,
    worst: AtomicU64,
    amp_min: AtomicU64,
    amp_max: AtomicU64,
    c: AtomicU64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            checked: AtomicU64::new(0),
            breaks: AtomicU64::new(0),
            worst: AtomicU64::new(0),
            amp_min: AtomicU64::new(f64::INFINITY.to_bits()),
            amp_max: AtomicU64::new(0),
            c: AtomicU64::new(f64::NAN.to_bits()),
        }
    }
}

/// A tone channel's figures over a stretch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Samples whose residual was checked (settled or within a click).
    pub checked: u64,
    pub breaks: u64,
    /// The worst residual over the tone's amplitude (0 with none checked).
    pub worst: f64,
    /// The least and largest level window amplitude, linear (infinity and 0
    /// with no window).
    pub amp_min: f64,
    pub amp_max: f64,
    /// The last fit's `2 cos w` (NaN before the first): [`hz`].
    pub c: f64,
}

impl Stats {
    /// The figures since the last take; the next stretch starts empty.
    pub fn take(&self) -> Summary {
        Summary {
            checked: self.checked.swap(0, Ordering::Relaxed),
            breaks: self.breaks.swap(0, Ordering::Relaxed),
            worst: f64::from_bits(self.worst.swap(0, Ordering::Relaxed)),
            amp_min: f64::from_bits(
                self.amp_min
                    .swap(f64::INFINITY.to_bits(), Ordering::Relaxed),
            ),
            amp_max: f64::from_bits(self.amp_max.swap(0, Ordering::Relaxed)),
            c: f64::from_bits(self.c.load(Ordering::Relaxed)),
        }
    }
}

/// The tone side's output for the run: its breaks and each channel's
/// figures.
pub struct Tones {
    pub events: ToneEvents,
    stats: Box<[Stats]>,
}

impl Tones {
    /// For `channels` tone channels.
    pub fn new(channels: usize) -> Self {
        Self {
            events: ToneEvents::default(),
            stats: (0..channels).map(|_| Stats::default()).collect(),
        }
    }

    /// The figures of tone channel `channel` since the last take.
    pub fn take(&self, channel: usize) -> Summary {
        self.stats[channel].take()
    }

    pub(super) fn stats(&self, channel: usize) -> &Stats {
        &self.stats[channel]
    }
}

/// Whether `v` lies beyond `thr`, either way.
pub fn above(v: f64, thr: f64) -> bool {
    v.abs() > thr
}

/// Whether a fit's `c` lies strictly between `lo` and `hi` (a NaN fit, as
/// of silence, never does).
pub fn in_band(c: f64, lo: f64, hi: f64) -> bool {
    lo < c && c < hi
}

/// Whether a tone's amplitude is above the gate, [`AMP_MIN`].
pub fn present(amp: f64) -> bool {
    amp > AMP_MIN
}

/// The break threshold of a tone of amplitude `amp`.
pub fn threshold(amp: f64) -> f64 {
    (amp * REL).max(QFLOOR)
}

/// Whether a fit's residual energy `rest` over [`ACQUIRE`] samples is a
/// clean tone's: its RMS at most half the threshold `thr`.
pub fn settles(rest: f64, thr: f64) -> bool {
    rest * 4.0 <= thr * thr * ACQUIRE as f64
}

/// Whether a window's amplitude over the running one, `ratio`, is a level
/// break (a NaN ratio is).
pub fn level_break(ratio: f64) -> bool {
    !(1.0 / LEVEL_STEP..=LEVEL_STEP).contains(&ratio)
}

/// The level window of a tone of `c`: its period rounded up, at least
/// [`LEVEL_MIN`] samples. A whole period keeps the quantisation's share in
/// the measured amplitude the same in every window.
pub fn window(c: f64) -> usize {
    ((TAU / (c / 2.0).acos()).ceil() as usize).max(LEVEL_MIN)
}

/// A ratio in dB.
pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The frequency of a tone of `c` at `rate`, Hz.
pub fn hz(c: f64, rate: u32) -> f64 {
    (c / 2.0).acos() * f64::from(rate) / TAU
}

/// The largest residual, over the tone's amplitude, of the phase jump a
/// repeated or skipped block of `block` samples leaves in a tone of `c`:
/// `2 |sin(block w / 2)|`, 0 when the block is a whole number of the
/// tone's periods (750 Hz in blocks of 128 at 96 kHz): the repeated block
/// is then the tone itself.
pub fn block_jump(c: f64, block: usize) -> f64 {
    2.0 * (block as f64 * (c / 2.0).acos() / 2.0).sin().abs()
}

/// Whether a block's phase jump of `jump` is under ten thresholds
/// (-40 dB): most repeated or skipped host blocks then go unseen.
pub fn blind(jump: f64) -> bool {
    jump < 10.0 * REL
}

/// What a rate sets: the band of `c` a tone may have, a click's longest
/// run and the running amplitude's step per sample.
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    c_lo: f64,
    c_hi: f64,
    max_len: u64,
    per_sample_run: f64,
}

impl Limits {
    pub(super) fn new(rate: u32) -> Self {
        let r = f64::from(rate.max(1));
        Self {
            // 2 cos w falls as w rises: the lowest tone is c's top.
            c_hi: 2.0 * (TAU * F_MIN / r).cos(),
            c_lo: 2.0 * (TAU * F_MAX).cos(),
            max_len: u64::from(rate.max(1)),
            per_sample_run: 1.0 / (RUN_S * r),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Acquire,
    Settled,
    Broken,
}

/// One tone channel's state; allocation-free after [`Channel::new`].
pub(super) struct Channel {
    /// The device's RX index.
    pub(super) rx: usize,
    /// The samples of history (up to two): a residual needs two.
    have: u8,
    /// The sample before and the one before it.
    p1: f64,
    p2: f64,
    mode: Mode,
    /// The fit: residuals so far and the sums of x^2, x y, y^2 and d^2
    /// (x the middle sample, y the two around it added, d subtracted).
    n: usize,
    sxx: f64,
    sxy: f64,
    syy: f64,
    sdd: f64,
    /// The settled tone: 2 cos w, 1 / (4 sin^2 w), the running amplitude,
    /// its inverse and the break threshold.
    c: f64,
    inv: f64,
    amp: f64,
    inv_amp: f64,
    thr: f64,
    /// The level window: its length, the running amplitude's step, the sum
    /// of squared amplitudes so far and their count.
    win: usize,
    alpha: f64,
    lsum: f64,
    ln: usize,
    /// The click: its first sample, worst residual, clean residuals in a
    /// row, the tone's peak over them and where they start.
    start: u64,
    worst: f64,
    ok: usize,
    okpeak: f64,
    clean_from: u64,
    /// This block's figures for [`Stats`].
    checked: u64,
    breaks: u64,
    rel_max: f64,
    amp_min: f64,
    amp_max: f64,
}

impl Channel {
    pub(super) fn new(rx: usize) -> Self {
        Self {
            rx,
            have: 0,
            p1: 0.0,
            p2: 0.0,
            mode: Mode::Acquire,
            n: 0,
            sxx: 0.0,
            sxy: 0.0,
            syy: 0.0,
            sdd: 0.0,
            c: f64::NAN,
            inv: 0.0,
            amp: 0.0,
            inv_amp: 0.0,
            thr: 0.0,
            win: LEVEL_MIN,
            alpha: 0.0,
            lsum: 0.0,
            ln: 0,
            start: 0,
            worst: 0.0,
            ok: 0,
            okpeak: 0.0,
            clean_from: 0,
            checked: 0,
            breaks: 0,
            rel_max: 0.0,
            amp_min: f64::INFINITY,
            amp_max: 0.0,
        }
    }

    /// Back to the start (a discontinuity), allocating nothing.
    pub(super) fn reset(&mut self) {
        self.have = 0;
        self.p1 = 0.0;
        self.p2 = 0.0;
        self.restart();
    }

    /// A new fit, keeping the samples before.
    fn restart(&mut self) {
        self.mode = Mode::Acquire;
        self.n = 0;
        self.sxx = 0.0;
        self.sxy = 0.0;
        self.syy = 0.0;
        self.sdd = 0.0;
    }

    fn set_amp(&mut self, amp: f64) {
        self.amp = amp;
        self.inv_amp = 1.0 / amp;
        self.thr = threshold(amp);
    }

    /// Sample `x` at media position `at`; breaks into `out` as channel `ch`.
    pub(super) fn feed(&mut self, x: f64, at: u64, k: &Limits, out: &ToneEvents, ch: usize) {
        if self.have < 2 {
            self.have += 1;
        } else {
            let y = x + self.p2;
            let d = x - self.p2;
            match self.mode {
                Mode::Acquire => self.acquire(y, d, k),
                Mode::Settled => self.settled(y, d, at, out, ch),
                Mode::Broken => self.broken(x, y, at, k, out, ch),
            }
        }
        self.p2 = self.p1;
        self.p1 = x;
    }

    fn acquire(&mut self, y: f64, d: f64, k: &Limits) {
        let x = self.p1;
        self.sxx += x * x;
        self.sxy += x * y;
        self.syy += y * y;
        self.sdd += d * d;
        self.n += 1;
        if self.n == ACQUIRE {
            self.settle(k);
        }
    }

    /// The fit's end: settled when it is a clean tone in the band, above the
    /// gate; a new fit otherwise.
    fn settle(&mut self, k: &Limits) {
        // Silence gives 0 / 0: a NaN fit, never in the band.
        let c = self.sxy / self.sxx;
        let inv = 1.0 / (4.0 - c * c);
        let amp = ((self.sxx + self.sdd * inv) / ACQUIRE as f64).sqrt();
        // The fit's residual energy, the sum of (y - c x)^2 at its c.
        let rest = (self.syy - c * self.sxy).max(0.0);
        let ok = in_band(c, k.c_lo, k.c_hi) && present(amp) && settles(rest, threshold(amp));
        self.restart();
        if ok {
            self.mode = Mode::Settled;
            self.c = c;
            self.inv = inv;
            self.set_amp(amp);
            self.win = window(c);
            self.alpha = self.win as f64 * k.per_sample_run;
            self.lsum = 0.0;
            self.ln = 0;
        }
    }

    /// A residual checked: counted, its size over the amplitude kept.
    fn check(&mut self, r: f64) {
        self.checked += 1;
        self.rel_max = self.rel_max.max(r.abs() * self.inv_amp);
    }

    fn settled(&mut self, y: f64, d: f64, at: u64, out: &ToneEvents, ch: usize) {
        let r = y - self.c * self.p1;
        self.check(r);
        if above(r, self.thr) {
            self.mode = Mode::Broken;
            self.start = at;
            self.worst = r.abs();
            self.ok = 0;
            self.okpeak = 0.0;
            return;
        }
        self.lsum += self.p1 * self.p1 + d * d * self.inv;
        self.ln += 1;
        if self.ln == self.win {
            self.level(at, out, ch);
        }
    }

    /// A level window's end at `at`.
    fn level(&mut self, at: u64, out: &ToneEvents, ch: usize) {
        let a = (self.lsum / self.win as f64).sqrt();
        self.amp_min = self.amp_min.min(a);
        self.amp_max = self.amp_max.max(a);
        self.lsum = 0.0;
        self.ln = 0;
        let ratio = a * self.inv_amp;
        if level_break(ratio) {
            out.push(ToneBreak {
                channel: ch,
                kind: ToneKind::Level,
                at: at + 1 - self.win as u64,
                len: self.win as u32,
                rel: ratio,
            });
            self.breaks += 1;
            self.restart();
            return;
        }
        self.set_amp(self.amp + (a - self.amp) * self.alpha);
        if !present(self.amp) {
            self.restart();
        }
    }

    fn broken(&mut self, x: f64, y: f64, at: u64, k: &Limits, out: &ToneEvents, ch: usize) {
        let r = y - self.c * self.p1;
        self.check(r);
        if above(r, self.thr) {
            self.ok = 0;
            self.okpeak = 0.0;
            self.worst = self.worst.max(r.abs());
        } else {
            // The first clean residual's oldest sample: the damage ended
            // before it.
            if self.ok == 0 {
                self.clean_from = at - 2;
            }
            self.ok += 1;
            self.okpeak = self.okpeak.max(x.abs());
        }
        let span = at + 1 - self.start;
        if self.ok >= RESUME && above(self.okpeak, self.thr) {
            self.end(self.clean_from.saturating_sub(self.start).max(1), out, ch);
        } else if span >= k.max_len {
            self.end(span, out, ch);
        }
    }

    /// The click ends, `len` samples long.
    fn end(&mut self, len: u64, out: &ToneEvents, ch: usize) {
        out.push(ToneBreak {
            channel: ch,
            kind: ToneKind::Click,
            at: self.start,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            rel: self.worst * self.inv_amp,
        });
        self.breaks += 1;
        self.restart();
    }

    /// This block's figures into `s`; the next block's start empty.
    pub(super) fn publish(&mut self, s: &Stats) {
        s.checked.fetch_add(self.checked, Ordering::Relaxed);
        s.breaks.fetch_add(self.breaks, Ordering::Relaxed);
        s.worst.fetch_max(self.rel_max.to_bits(), Ordering::Relaxed);
        s.amp_min
            .fetch_min(self.amp_min.to_bits(), Ordering::Relaxed);
        s.amp_max
            .fetch_max(self.amp_max.to_bits(), Ordering::Relaxed);
        s.c.store(self.c.to_bits(), Ordering::Relaxed);
        self.checked = 0;
        self.breaks = 0;
        self.rel_max = 0.0;
        self.amp_min = f64::INFINITY;
        self.amp_max = 0.0;
    }
}

#[cfg(test)]
mod tests;
