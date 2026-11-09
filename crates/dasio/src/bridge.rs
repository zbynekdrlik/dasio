//! The pump's processor: double-buffered, as a sound card's ASIO driver.
//!
//! Per block `seq`: the RX samples into half `seq % 2`'s input buffers (24
//! bits left-justified in `i32`, as ASIO's `Int32LSB`), `seq` published and
//! the block event signalled; the outputs are the DAW's answer to the block
//! before (half `(seq - 1) % 2`), taken when its `ack` is in. The block's
//! outputs leave the work margin W after the pump calls it, and the output
//! latency already counts W (`latencies`): with a deadline set
//! ([`Bridge::with_deadline`]) the bridge waits for a missing answer until
//! then, so the DAW has a block period plus most of W. A missing answer is
//! silence and a late count. The cost is one block on the way out.
//!
//! With concealment ([`Bridge::with_concealment`]), the gap has no steps:
//! the first late block plays the last played block time-reversed and
//! faded out (it starts at the last sample played and ends at 0), further
//! late blocks are silence, and the first answer after the gap fades in.
//! A hard cut to silence and back was a click at each edge of every gap
//! (dasio#1, the 8.11.2026 service).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dstack_audio_io::{Block, Process};

/// The DAW side of the shared memory, as the bridge uses it (the mapping on
/// Windows, a stand-in in the tests).
pub trait Link: Send {
    /// A DAW holds the buffers.
    fn attached(&self) -> bool;
    /// The `ch`th input buffer of `half`.
    fn input(&mut self, half: u32, ch: usize) -> &mut [i32];
    /// The `ch`th output buffer of `half` into `dst` (f64, ±1 full scale).
    fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]);
    /// Publishes block `seq` (its inputs are in) and wakes the driver;
    /// `start` is the media time of its first input sample (the pump's
    /// [`Block::start`], `None` without a clock).
    fn publish(&self, seq: u64, start: Option<u64>);
    /// The newest block the DAW returned.
    fn ack(&self) -> u64;
    /// One block that came too late.
    fn count_late(&self);
}

/// f64 (±1 full scale) to 24 bits left-justified, as the pump reads them.
pub fn to_i32(x: f64) -> i32 {
    (x * 2_147_483_648.0)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// 24 bits left-justified to f64 (±1 full scale), exact.
pub fn to_f64(s: i32) -> f64 {
    f64::from(s) / 2_147_483_648.0
}

/// The media clock now, samples (`None` while it is not ready).
type MediaNow = Box<dyn Fn() -> Option<u64> + Send>;

/// The pump's processor for one instance.
pub struct Bridge<L: Link> {
    link: L,
    seq: u64,
    /// Block periods the DAW gets beyond the first (0 or more): its
    /// outputs play that many blocks later.
    spare: u64,
    /// Answers dropped because their slot had passed (see `process`).
    stale: Arc<AtomicU64>,
    /// The media start of the newest published blocks, by seq (`spare` + 2
    /// of them): an answer plays only in the block `spare` + 1 blocks
    /// after its own.
    starts: Vec<Option<u64>>,
    /// The latest media time (samples after a block's start) the bridge
    /// waits for a missing answer until, and the media clock (`None`: not
    /// ready); `None`: it never waits.
    deadline: Option<(u64, MediaNow)>,
    /// Answers that came in during that wait (late without it).
    waited: Arc<AtomicU64>,
    /// The concealment of late blocks; `None`: a late block is silence.
    conceal: Option<Conceal>,
}

/// The last played block and the fades of a gap ([`Bridge::with_concealment`]),
/// all allocated before the pump runs (its thread must not allocate).
struct Conceal {
    /// The last block played, channel-major (`outputs` × `frames`).
    last: Vec<f64>,
    frames: usize,
    /// Raised-cosine fade-out over a block: 1 at the first sample, ~0 at
    /// the last.
    fade_out: Vec<f64>,
    /// Raised-cosine fade-in over a block: ~0 at the first sample, 1 at
    /// the last.
    fade_in: Vec<f64>,
    /// The block before was late: the next answer fades in.
    in_gap: bool,
}

impl Conceal {
    fn new(outputs: usize, frames: usize) -> Self {
        let n = frames.max(1) as f64;
        let pi = std::f64::consts::PI;
        Conceal {
            last: vec![0.0; outputs * frames],
            frames,
            fade_out: (0..frames)
                .map(|i| 0.5 * (1.0 + (pi * i as f64 / n).cos()))
                .collect(),
            fade_in: (0..frames)
                .map(|i| 0.5 * (1.0 - (pi * (i + 1) as f64 / n).cos()))
                .collect(),
            in_gap: false,
        }
    }

    /// Whether `block` has the shape this was made for.
    fn fits(&self, block: &Block<'_>) -> bool {
        block.frames() == self.frames && block.outputs() * self.frames == self.last.len()
    }

    /// `block` plays an answer: faded in after a gap, then kept.
    fn played(&mut self, block: &mut Block<'_>) {
        if !self.fits(block) {
            return;
        }
        for ch in 0..block.outputs() {
            let out = block.output(ch);
            if self.in_gap {
                for (y, &g) in out.iter_mut().zip(&self.fade_in) {
                    *y *= g;
                }
            }
            let base = ch * self.frames;
            self.last[base..base + self.frames].copy_from_slice(out);
        }
        self.in_gap = false;
    }

    /// `block` has no answer: the gap's first block fades the last one out
    /// backwards, from its last sample; later ones stay silence.
    fn missed(&mut self, block: &mut Block<'_>) {
        if !self.fits(block) {
            return;
        }
        if !self.in_gap {
            for ch in 0..block.outputs() {
                let base = ch * self.frames;
                let last = &self.last[base..base + self.frames];
                for ((y, &s), &g) in block
                    .output(ch)
                    .iter_mut()
                    .zip(last.iter().rev())
                    .zip(&self.fade_out)
                {
                    *y = s * g;
                }
            }
        }
        self.in_gap = true;
    }

    /// No DAW holds the buffers: the pump plays silence, so the first
    /// answer after it comes back fades in.
    fn detached(&mut self) {
        self.in_gap = true;
    }

    /// The engine reopened (it fades in by itself): forget the last block.
    fn forget(&mut self) {
        self.last.fill(0.0);
        self.in_gap = false;
    }
}

impl<L: Link> Bridge<L> {
    pub fn new(link: L) -> Self {
        Self::with_spare(link, 0)
    }

    /// A bridge whose DAW gets `spare` more block periods to return a
    /// block: its outputs are taken `spare` blocks later. Both halves stay
    /// safe with one spare block: a block's outputs are read before the
    /// next block of the same half is published, and its inputs are only
    /// overwritten after the DAW returned it (or it is late anyway).
    pub fn with_spare(link: L, spare: u64) -> Self {
        Bridge {
            link,
            seq: 0,
            spare,
            stale: Arc::new(AtomicU64::new(0)),
            starts: vec![None; spare as usize + 2],
            deadline: None,
            waited: Arc::new(AtomicU64::new(0)),
            conceal: None,
        }
    }

    /// A bridge that conceals late blocks (see the module docs) for
    /// blocks of `outputs` channels of `frames` samples; a block of
    /// another shape is not concealed. Allocates here, never in `process`.
    pub fn with_concealment(mut self, outputs: usize, frames: usize) -> Self {
        self.conceal = Some(Conceal::new(outputs, frames));
        self
    }

    /// A bridge that waits for a missing answer until `offset` samples
    /// after the start of the block being processed, by the media clock
    /// `now` (samples; `None` while it is not ready). The pump's block is
    /// due B + P after its start and its outputs leave W later, so the
    /// offset is B + P + W less a safety margin for the pump's own work.
    pub fn with_deadline(
        mut self,
        offset: u64,
        now: impl Fn() -> Option<u64> + Send + 'static,
    ) -> Self {
        self.deadline = Some((offset, Box::new(now)));
        self
    }

    /// The count of answers that came in during the wait, shared.
    pub fn waited_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.waited)
    }

    /// Waits until the DAW returned block `due` or the deadline after
    /// `start` passed; whether it had to wait and the answer came.
    fn wait_for(&self, due: u64, start: Option<u64>) -> bool {
        let (Some((offset, now)), Some(start)) = (&self.deadline, start) else {
            return false;
        };
        if self.link.ack() >= due {
            return false;
        }
        let until = start + offset;
        while self.link.ack() < due {
            match now() {
                Some(t) if t < until => std::hint::spin_loop(),
                _ => return false,
            }
        }
        true
    }

    /// Takes block `due`'s answer into `block`'s outputs, or counts it late
    /// or stale.
    fn take(&mut self, due: u64, block: &mut Block<'_>) {
        if self.wait_for(due, block.start()) {
            self.waited.fetch_add(1, Ordering::Relaxed);
        }
        // Its answer's own slot is `spare` + 1 blocks after it. A pump
        // that skipped blocks (a re-bootstrap) runs this block later: the
        // answer would play that many blocks late, outside the latency the
        // driver reports, so it is dropped. Without media times (free
        // pacing) every block is its slot.
        let len = self.starts.len();
        let in_slot = match (block.start(), self.starts[due as usize % len]) {
            (Some(now), Some(then)) => now == then + (self.spare + 1) * block.frames() as u64,
            _ => true,
        };
        if self.link.ack() >= due && in_slot {
            let half = (due % 2) as u32;
            for ch in 0..block.outputs() {
                self.link.read_output(half, ch, block.output(ch));
            }
            if let Some(c) = self.conceal.as_mut() {
                c.played(block);
            }
            return;
        }
        if self.link.ack() >= due {
            self.stale.fetch_add(1, Ordering::Relaxed);
        } else {
            self.link.count_late();
        }
        if let Some(c) = self.conceal.as_mut() {
            c.missed(block);
        }
    }

    /// Hands `block`'s inputs to the DAW as the next seq.
    fn hand_over(&mut self, block: &Block<'_>) {
        self.seq += 1;
        let half = (self.seq % 2) as u32;
        for ch in 0..block.inputs() {
            let src = block.input(ch);
            for (d, &x) in self.link.input(half, ch).iter_mut().zip(src) {
                *d = to_i32(x);
            }
        }
        let len = self.starts.len();
        self.starts[self.seq as usize % len] = block.start();
        self.link.publish(self.seq, block.start());
    }

    /// The count of answers dropped because their slot had passed, shared
    /// (the pump owns the bridge; the counts line reads it).
    pub fn stale_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.stale)
    }
}

impl<L: Link> Process for Bridge<L> {
    fn discontinuity(&mut self) {
        if let Some(c) = self.conceal.as_mut() {
            c.forget();
        }
    }

    fn process(&mut self, block: &mut Block<'_>) {
        if !self.link.attached() {
            if let Some(c) = self.conceal.as_mut() {
                c.detached();
            }
            return;
        }
        // The block whose outputs are due now.
        let due = self.seq.saturating_sub(self.spare);
        // Without a spare block the DAW gets this block before the bridge
        // waits for the last one's answer: they use different halves. With
        // one, the due block's outputs half is the one this block's answer
        // goes to, so it is read first.
        let early = self.spare == 0;
        if early {
            self.hand_over(block);
        }
        if due > 0 {
            self.take(due, block);
        }
        if !early {
            self.hand_over(block);
        }
    }
}

#[cfg(windows)]
pub use win::SharedLink;

#[cfg(windows)]
mod win {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use shm::Shared;

    use super::Link;

    /// The instance's mapping as the bridge's link; each hand-over's time
    /// (`shm::qpc_ns`, the driver's clock too) goes to `stamps` for the
    /// turnaround watcher.
    pub struct SharedLink {
        pub shared: Arc<Shared>,
        pub stamps: Arc<crate::turnaround::Stamps>,
    }

    impl Link for SharedLink {
        fn attached(&self) -> bool {
            self.shared.header().host_attached.load(Ordering::Acquire) != 0
        }

        fn input(&mut self, half: u32, ch: usize) -> &mut [i32] {
            let n = self.shared.shape().block as usize;
            let p = self
                .shared
                .buffer(half, true, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; this half is the
            // device's until it publishes it.
            unsafe { std::slice::from_raw_parts_mut(p, n) }
        }

        fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]) {
            let n = self.shared.shape().block as usize;
            let p = self
                .shared
                .buffer(half, false, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; the DAW returned them.
            let src = unsafe { std::slice::from_raw_parts(p, n) };
            for (y, &s) in dst.iter_mut().zip(src) {
                *y = super::to_f64(s);
            }
        }

        fn publish(&self, seq: u64, start: Option<u64>) {
            let h = self.shared.header();
            // The half's media time before its seq: a driver that sees the
            // seq sees the start.
            h.start[(seq % 2) as usize].store(start.unwrap_or(shm::NO_START), Ordering::Relaxed);
            self.stamps.publish(seq, shm::qpc_ns());
            h.seq.store(seq, Ordering::Release);
            self.shared.signal_block();
        }

        fn ack(&self) -> u64 {
            self.shared.header().ack.load(Ordering::Acquire)
        }

        fn count_late(&self) {
            self.shared.header().late.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
