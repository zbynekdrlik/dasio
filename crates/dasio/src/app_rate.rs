// SPDX-License-Identifier: GPL-3.0-or-later

//! An ASIO app at its own sample rate on the device's 96 kHz (dasio#107;
//! owner, 13.11.2026: the Dante rate stays read-only, one rate for the
//! whole network, and an app at another rate is converted locally).
//!
//! The pump keeps the device's block B at the network's rate. For an app
//! at another rate R the instance's processor is
//! `Rated<Reblock<Bridge>>` ([`Served`]):
//!
//! - dlink's [`Rated`] (dstack #90) converts the device's RX block into
//!   about `B * R / 96000` app frames and the app's output back into
//!   exactly B device frames, on the device's one media clock (a fixed,
//!   exact ratio). 48 kHz steps through dstack-resample's half-band (an
//!   exact 2:1 on an even block), 44.1 kHz through its sinc.
//! - [`Reblock`] gives the bridge (and so the DAW) a FIXED block of
//!   `A = ceil(B * R / 96000)` app frames, as ASIO's buffer size is fixed:
//!   at 44.1 kHz [`Rated`] gives 58 or 59 frames a device block. The DAW's
//!   answers wait in a FIFO primed with A frames, so the frame-to-frame
//!   variation never runs it dry. At 48 kHz every device block gives
//!   exactly A frames: one DAW block a device block, no prime, no residue.
//! - The bridge is the 96 kHz bridge unchanged, at the app's block and
//!   rate: its media starts are in app samples ([`Rated`] keeps them
//!   contiguous), its deadline on the media clock at the app's rate.
//!
//! At the device's own rate the processor is the bridge alone, as before.
//! [`Plan`] holds every decision (the app's block, whether the ratio is
//! exact, the latencies the driver reports) as plain arithmetic.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dlink::rated::Rated;
use dstack_audio_io::{Block, Process};
use dstack_resample::{Converter, Fixed};

use crate::bridge::WaitFrom;

/// How an instance serves an app at `app_rate` on a device at
/// `device_rate` running `device_block` frames a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub device_rate: u32,
    pub app_rate: u32,
    pub device_block: u32,
    /// The DAW's block, app frames: the device block's period at the
    /// app's rate, rounded up (`ceil(B * R / D)`), so a device block never
    /// gives more than one DAW block's frames.
    pub app_block: u32,
    /// Every device block gives exactly `app_block` app frames (the same
    /// rate, or an exact 2:1 on an even block, the half-band): one DAW
    /// block per device block, no FIFO prime and no residue.
    pub exact: bool,
}

impl Plan {
    /// The plan for `app_rate`; `None` for a rate apps may not ask
    /// ([`shm::APP_RATES`]; the device's own rate always serves), one above
    /// the device's, or an app block the shared memory cannot carry.
    pub fn new(device_rate: u32, app_rate: u32, device_block: u32) -> Option<Plan> {
        let asked_ok = app_rate == device_rate || shm::APP_RATES.contains(&app_rate);
        if !asked_ok || app_rate == 0 || app_rate > device_rate || device_block == 0 {
            return None;
        }
        let app_block = u64::from(device_block) * u64::from(app_rate);
        let app_block = u32::try_from(app_block.div_ceil(u64::from(device_rate))).ok()?;
        shm::Shape::app(0, 0, app_block)?;
        let exact = app_rate == device_rate
            || (2 * u64::from(app_rate) == u64::from(device_rate)
                && device_block.is_multiple_of(2));
        Some(Plan {
            device_rate,
            app_rate,
            device_block,
            app_block,
            exact,
        })
    }

    /// The app runs at another rate than the device: the processor
    /// converts.
    pub fn converts(&self) -> bool {
        self.app_rate != self.device_rate
    }

    /// The silence the reblock's TX FIFO starts with, app frames: none on
    /// an exact ratio, else a whole DAW block (the most the RX side can
    /// hold back, `app_block - 1` frames, never runs it dry).
    pub fn prime(&self) -> usize {
        if self.exact {
            0
        } else {
            self.app_block as usize
        }
    }

    /// `samples` at the device's rate in app samples, rounded up.
    pub fn to_app(&self, samples: u64) -> u64 {
        (u128::from(samples) * u128::from(self.app_rate))
            .div_ceil(u128::from(self.device_rate.max(1))) as u64
    }

    /// `ns` in app samples, rounded up.
    pub fn ns_to_app(&self, ns: u64) -> u64 {
        (u128::from(ns) * u128::from(self.app_rate)).div_ceil(1_000_000_000) as u64
    }

    /// The reblock's share of the input and the output latency, app
    /// frames. On a ratio that is not exact the round trip gains the TX
    /// FIFO's prime, one DAW block: a DAW block's inputs wait in the RX
    /// FIFO for the residue (0 to `app_block - 1` frames, block to block),
    /// half a block on the mean, and its outputs the rest of the prime.
    /// None on an exact ratio.
    pub fn reblock_share(&self) -> (u64, u64) {
        if self.exact {
            return (0, 0);
        }
        let a = u64::from(self.app_block);
        (a / 2, a - a / 2)
    }

    /// The DAW's input and output latency, app samples: the device's
    /// (`device`, samples at its rate: B + P in, B + W + TX latency +
    /// spare blocks out, [`crate::latencies`]) at the app's rate, plus the
    /// converter's delay each way (`conversion_ns`, RX then TX:
    /// [`dlink::rated::delay_ns`]) and the reblock's share. Exact at 48 kHz;
    /// at 44.1 kHz the round trip is exact to a sample or two (the pulse
    /// test measures it) and the in/out split of the reblock's share is
    /// its mean, as the residue varies block to block.
    pub fn latencies(&self, device: (u32, u32), conversion_ns: (u64, u64)) -> (u32, u32) {
        if !self.converts() {
            return device;
        }
        let (share_in, share_out) = self.reblock_share();
        let way = |samples: u32, ns: u64, share: u64| {
            let total = self
                .to_app(u64::from(samples))
                .saturating_add(self.ns_to_app(ns))
                .saturating_add(share);
            u32::try_from(total).unwrap_or(u32::MAX)
        };
        (
            way(device.0, conversion_ns.0, share_in),
            way(device.1, conversion_ns.1, share_out),
        )
    }

    /// How long after a DAW block's start the bridge may wait for its
    /// answer, app samples: the device's offset (`device_offset`, its
    /// samples, [`crate::wait_offset`]) at the app's rate, rounded down,
    /// so the wait never passes the device block's own deadline. At 44.1
    /// kHz a DAW block starts up to a block before the device block that
    /// completes it, so its wait ends up to that much early (more late
    /// blocks under load, never a late output); at 48 kHz none.
    pub fn wait_offset(&self, device_offset: u64) -> u64 {
        (u128::from(device_offset) * u128::from(self.app_rate)
            / u128::from(self.device_rate.max(1))) as u64
    }

    /// The DAW block's period, ns.
    pub fn period_ns(&self) -> u64 {
        u64::from(self.app_block) * 1_000_000_000 / u64::from(self.app_rate.max(1))
    }
}

/// The plan to serve next when the DAW asked `asked` Hz (0: nothing asked)
/// while the instance serves `now`: `Some` only for a supported rate other
/// than the one served.
pub fn switch_to(asked: u32, now: &Plan) -> Option<Plan> {
    if asked == 0 || asked == now.app_rate {
        return None;
    }
    Plan::new(now.device_rate, asked, now.device_block)
}

/// The plan an instance starts with: the rate a DAW asked before (it
/// stays in the instance's directory across device restarts) when it is
/// supported, else the device's own.
pub fn first_plan(device_rate: u32, device_block: u32, asked: u32) -> Option<Plan> {
    Plan::new(device_rate, asked, device_block)
        .or_else(|| Plan::new(device_rate, device_rate, device_block))
}

/// Fills `dst` from the front of `fifo` and the rest with silence; the
/// frames it had to fill with silence.
fn drain_into(fifo: &mut VecDeque<f64>, dst: &mut [f64]) -> usize {
    let have = fifo.len().min(dst.len());
    for (d, v) in dst.iter_mut().zip(fifo.drain(..have)) {
        *d = v;
    }
    dst[have..].fill(0.0);
    dst.len() - have
}

/// Fixed blocks for an inner processor from variable ones (see the module
/// docs): the inputs gather until a whole block of `block` frames is in,
/// the inner processor's outputs go out through a FIFO primed with
/// `prime` frames of silence. Every buffer is allocated in [`Reblock::new`].
pub struct Reblock<P> {
    inner: P,
    block: usize,
    prime: usize,
    /// Input frames gathered, per input channel, and how many (also with
    /// no input channel).
    rx: Vec<VecDeque<f64>>,
    held: usize,
    /// The media start of the oldest gathered frame.
    rx_start: Option<u64>,
    /// The inner processor's outputs waiting to go out, per channel.
    tx: Vec<VecDeque<f64>>,
    /// One inner block's inputs and outputs, channel-major.
    staged_in: Vec<f64>,
    staged_out: Vec<f64>,
    /// Output frames the TX FIFO could not give (played as silence).
    dry: Arc<AtomicU64>,
    /// Blocks whose start did not follow on (the FIFOs started again).
    restarts: Arc<AtomicU64>,
    /// Set to each outer block's start before its inner blocks run: an
    /// inner block may start up to a block before it, and the bridge's wait
    /// counts from the device block's start.
    wait_from: Option<WaitFrom>,
}

impl<P: Process> Reblock<P> {
    /// `inner` on blocks of `block` frames of `inputs` and `outputs`
    /// channels, its outputs behind `prime` frames of silence; the outer
    /// blocks are at most `most` frames.
    pub fn new(
        inner: P,
        block: usize,
        prime: usize,
        inputs: usize,
        outputs: usize,
        most: usize,
    ) -> Self {
        let block = block.max(1);
        // The most each FIFO ever holds: under a block gathered plus an
        // outer block (RX); the prime plus an outer block (TX).
        let room = block + most + prime;
        let mut tx: Vec<VecDeque<f64>> = (0..outputs)
            .map(|_| VecDeque::with_capacity(room))
            .collect();
        for q in &mut tx {
            q.extend(std::iter::repeat_n(0.0, prime));
        }
        Reblock {
            inner,
            block,
            prime,
            rx: (0..inputs).map(|_| VecDeque::with_capacity(room)).collect(),
            held: 0,
            rx_start: None,
            tx,
            staged_in: vec![0.0; inputs * block],
            staged_out: vec![0.0; outputs * block],
            dry: Arc::new(AtomicU64::new(0)),
            restarts: Arc::new(AtomicU64::new(0)),
            wait_from: None,
        }
    }

    /// This reblock setting `wait_from` to each outer block's start before
    /// it runs that block's inner blocks (the bridge's
    /// [`crate::bridge::Bridge::wait_from`]).
    pub fn with_wait_from(mut self, wait_from: WaitFrom) -> Self {
        self.wait_from = Some(wait_from);
        self
    }

    /// The output frames played as silence because the FIFO was dry,
    /// shared.
    pub fn dry(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dry)
    }

    /// The times the FIFOs started again (a jump in the blocks' starts),
    /// shared.
    pub fn restarts(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.restarts)
    }

    /// The FIFOs from the start: nothing gathered, the TX FIFO primed.
    fn restart(&mut self) {
        for q in &mut self.rx {
            q.clear();
        }
        self.held = 0;
        self.rx_start = None;
        for q in &mut self.tx {
            q.clear();
            q.extend(std::iter::repeat_n(0.0, self.prime));
        }
    }

    /// One inner block from the gathered frames.
    fn inner_block(&mut self) {
        let n = self.block;
        for (c, q) in self.rx.iter_mut().enumerate() {
            for (d, v) in self.staged_in[c * n..(c + 1) * n]
                .iter_mut()
                .zip(q.drain(..n))
            {
                *d = v;
            }
        }
        // The inner processor writes only what it answers (the bridge
        // leaves a missing answer's block alone).
        self.staged_out.fill(0.0);
        {
            let mut b = Block::new(n, &self.staged_in, &mut self.staged_out);
            if let Some(s) = self.rx_start {
                b = b.at(s);
            }
            self.inner.process(&mut b);
        }
        for (c, q) in self.tx.iter_mut().enumerate() {
            q.extend(self.staged_out[c * n..(c + 1) * n].iter().copied());
        }
        self.held -= n;
        self.rx_start = self.rx_start.map(|s| s + n as u64);
    }
}

impl<P: Process> Process for Reblock<P> {
    fn process(&mut self, block: &mut Block<'_>) {
        let k = block.frames();
        // A block that does not follow the gathered frames (the device
        // skipped blocks): what was gathered belongs to another time.
        let follows = match (block.start(), self.rx_start) {
            (Some(s), Some(first)) => s == first + self.held as u64,
            _ => true,
        };
        if !follows {
            self.restart();
            self.restarts.fetch_add(1, Ordering::Relaxed);
        }
        if self.held == 0 {
            self.rx_start = block.start();
        }
        for (c, q) in self.rx.iter_mut().enumerate() {
            let input = block.input(c);
            if input.len() == k {
                q.extend(input.iter().copied());
            } else {
                // A channel the block lacks: silence, so every channel
                // holds the same frames.
                q.extend(std::iter::repeat_n(0.0, k));
            }
        }
        self.held += k;
        if let Some(w) = &self.wait_from {
            w.set(block.start());
        }
        // The whole blocks gathered, counted first: the loop is bounded by
        // the frames in, whatever an inner block does.
        for _ in 0..self.held / self.block {
            self.inner_block();
        }
        // Every channel's FIFO holds the same frames: channel 0 counts.
        let mut short = 0;
        for (c, q) in self.tx.iter_mut().enumerate() {
            let s = drain_into(q, block.output(c));
            if c == 0 {
                short = s;
            }
        }
        self.dry.fetch_add(short as u64, Ordering::Relaxed);
    }

    fn discontinuity(&mut self) {
        self.restart();
        self.inner.discontinuity();
    }
}

/// The instance's processor for a [`Plan`]: the bridge alone at the
/// device's rate, behind the converters and the reblock at another.
pub enum Served<P> {
    Direct(P),
    Converted(Box<Rated<Reblock<P>>>),
}

/// The counters of a converted path: the converter's TX FIFO's dry frames
/// and the reblock's (both played as silence), and the reblock's restarts.
#[derive(Debug, Clone, Default)]
pub struct Counters {
    pub converter_dry: Option<Arc<AtomicU64>>,
    pub reblock_dry: Option<Arc<AtomicU64>>,
    pub reblock_restarts: Option<Arc<AtomicU64>>,
}

impl Counters {
    /// (converter dry, reblock dry, reblock restarts), 0 without a
    /// converter.
    pub fn read(&self) -> (u64, u64, u64) {
        let get = |c: &Option<Arc<AtomicU64>>| c.as_ref().map_or(0, |a| a.load(Ordering::Relaxed));
        (
            get(&self.converter_dry),
            get(&self.reblock_dry),
            get(&self.reblock_restarts),
        )
    }
}

impl<P: Process> Served<P> {
    /// `inner` (the bridge, at the plan's app block) served by `plan` for
    /// `inputs` RX and `outputs` TX channels; the converter's refusal as
    /// text. On a converted path the reblock sets `wait_from` (the bridge's
    /// own, [`crate::bridge::Bridge::wait_from`]) to each device block's
    /// start in app samples; the direct path leaves it unset.
    pub fn new(
        inner: P,
        plan: &Plan,
        inputs: usize,
        outputs: usize,
        wait_from: WaitFrom,
    ) -> Result<(Served<P>, Counters), String> {
        if !plan.converts() {
            return Ok((Served::Direct(inner), Counters::default()));
        }
        let app_block = plan.app_block as usize;
        // The most app frames a device block gives: the RX converter's own
        // bound (Rated makes its RX converter the same way), so the
        // reblock's FIFOs never grow on the pump's thread.
        let most = Converter::low_delay(
            plan.device_rate,
            plan.app_rate,
            1,
            plan.device_block as usize,
            Fixed::Input,
        )
        .map_err(|e| e.to_string())?
        .output_frames_max();
        let reblock = Reblock::new(inner, app_block, plan.prime(), inputs, outputs, most)
            .with_wait_from(wait_from);
        let counters_reblock = (reblock.dry(), reblock.restarts());
        let rated = Rated::new(
            reblock,
            plan.device_rate,
            plan.app_rate,
            plan.device_block as usize,
            inputs,
            outputs,
        )
        .map_err(|e| e.to_string())?;
        let counters = Counters {
            converter_dry: Some(rated.dry()),
            reblock_dry: Some(counters_reblock.0),
            reblock_restarts: Some(counters_reblock.1),
        };
        Ok((Served::Converted(Box::new(rated)), counters))
    }
}

impl<P: Process> Process for Served<P> {
    #[cfg_attr(dasio_rtsan, sanitize(realtime = "nonblocking"))]
    fn process(&mut self, block: &mut Block<'_>) {
        match self {
            Served::Direct(p) => p.process(block),
            Served::Converted(p) => p.process(block),
        }
    }

    fn discontinuity(&mut self) {
        match self {
            Served::Direct(p) => p.discontinuity(),
            Served::Converted(p) => p.discontinuity(),
        }
    }
}

/// The converter's delay each way, ns (RX, TX), for `plan`: (0, 0) at the
/// device's rate; the converter's refusal as text.
pub fn conversion_ns(plan: &Plan) -> Result<(u64, u64), String> {
    dlink::rated::delay_ns(plan.device_rate, plan.app_rate, plan.device_block as usize)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
