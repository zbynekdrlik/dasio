//! The pump's processor: double-buffered, as a sound card's ASIO driver.
//!
//! Per block `seq`: the RX samples into half `seq % 2`'s input buffers (24
//! bits left-justified in `i32`, as ASIO's `Int32LSB`), `seq` published and
//! the block event signalled; the outputs are the DAW's answer to the block
//! before (half `(seq - 1) % 2`), taken when its `ack` is in. The pump never
//! waits: the DAW has one block's time to answer, and a missing answer is
//! silence and a late count. The cost is one block on the way out
//! (`latencies` counts it).

use audio_io::{Block, Process};

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

/// The pump's processor for one instance.
pub struct Bridge<L: Link> {
    link: L,
    seq: u64,
}

impl<L: Link> Bridge<L> {
    pub fn new(link: L) -> Self {
        Bridge { link, seq: 0 }
    }
}

impl<L: Link> Process for Bridge<L> {
    fn process(&mut self, block: &mut Block<'_>) {
        if !self.link.attached() {
            return;
        }
        let prev = self.seq;
        if prev > 0 {
            if self.link.ack() >= prev {
                let half = (prev % 2) as u32;
                for ch in 0..block.outputs() {
                    self.link.read_output(half, ch, block.output(ch));
                }
            } else {
                self.link.count_late();
            }
        }
        self.seq += 1;
        let half = (self.seq % 2) as u32;
        for ch in 0..block.inputs() {
            let src = block.input(ch);
            for (d, &x) in self.link.input(half, ch).iter_mut().zip(src) {
                *d = to_i32(x);
            }
        }
        self.link.publish(self.seq, block.start());
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

    /// The instance's mapping as the bridge's link.
    pub struct SharedLink(pub Arc<Shared>);

    impl Link for SharedLink {
        fn attached(&self) -> bool {
            self.0.header().host_attached.load(Ordering::Acquire) != 0
        }

        fn input(&mut self, half: u32, ch: usize) -> &mut [i32] {
            let n = self.0.shape().block as usize;
            let p = self
                .0
                .buffer(half, true, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; this half is the
            // device's until it publishes it.
            unsafe { std::slice::from_raw_parts_mut(p, n) }
        }

        fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]) {
            let n = self.0.shape().block as usize;
            let p = self
                .0
                .buffer(half, false, ch as u32)
                .expect("a channel of the shape");
            // SAFETY: `n` samples inside the mapping; the DAW returned them.
            let src = unsafe { std::slice::from_raw_parts(p, n) };
            for (y, &s) in dst.iter_mut().zip(src) {
                *y = super::to_f64(s);
            }
        }

        fn publish(&self, seq: u64, start: Option<u64>) {
            let h = self.0.header();
            // The half's media time before its seq: a driver that sees the
            // seq sees the start.
            h.start[(seq % 2) as usize].store(start.unwrap_or(shm::NO_START), Ordering::Relaxed);
            h.seq.store(seq, Ordering::Release);
            self.0.signal_block();
        }

        fn ack(&self) -> u64 {
            self.0.header().ack.load(Ordering::Acquire)
        }

        fn count_late(&self) {
            self.0.header().late.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
