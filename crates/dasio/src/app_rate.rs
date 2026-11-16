// SPDX-License-Identifier: GPL-3.0-or-later

//! An ASIO app at its own sample rate on the device's 96 kHz (dasio#107;
//! owner, 13.11.2026: the Dante rate stays read-only, one rate for the
//! whole network, and an app at another rate is converted locally).
//!
//! The pump keeps the device's block B at the network's rate. For an app
//! at another rate R the instance's processor is dlink's [`Rated`] around
//! the bridge, with a fixed app block ([`Served`]):
//!
//! - [`Rated`] (dstack #90) converts the device's RX block into about
//!   `B * R / 96000` app frames and the app's output back into exactly B
//!   device frames, on the device's one media clock (a fixed, exact
//!   ratio). 48 kHz steps through dstack-resample's half-band (an exact 2:1
//!   on an even block), 44.1 kHz through its sinc.
//! - Made with [`Rated::with_fixed_app_block`] (dstack #330), it gives the
//!   bridge (and so the DAW) a FIXED block of `A = ceil(B * R / 96000)` app
//!   frames, as ASIO's buffer size is fixed: at 44.1 kHz the converter
//!   gives 58 or 59 frames a device block, which wait in Rated's RX FIFO
//!   until A are in (0 or 1 DAW blocks a device block), and its TX FIFO
//!   starts with A more frames of silence. At 48 kHz every device block
//!   gives exactly A frames: one DAW block a device block, no FIFO, no
//!   residue.
//! - The bridge is the 96 kHz bridge unchanged, at the app's block and
//!   rate: its media starts are in app samples ([`Rated`] keeps them
//!   contiguous), its deadline on the media clock at the app's rate. Its
//!   wait counts from the device block's start ([`WaitFrom`], set before
//!   [`Rated`] runs that device block): a DAW block starts up to a block
//!   before it at 44.1 kHz.
//!
//! At the device's own rate the processor is the bridge alone, as before.
//! [`Plan`] holds every decision (the app's block, whether the ratio is
//! exact, the latencies the driver reports) as plain arithmetic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dlink::rated::Rated;
use dstack_audio_io::{Block, Process};

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

    /// The silence the fixed app block adds to [`Rated`]'s TX FIFO, app
    /// frames (beyond its own `dlink::rated::PRIME`, which
    /// [`conversion_ns`] holds): none on an exact ratio, else a whole DAW
    /// block (its RX FIFO holds back up to `app_block - 1` frames, dstack
    /// #330).
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

    /// The fixed app block's share of the input and the output latency,
    /// app frames. On a ratio that is not exact the round trip gains its
    /// prime, one DAW block: a DAW block's inputs wait in Rated's RX FIFO
    /// for the residue (0 to `app_block - 1` frames, block to block), half
    /// a block on the mean, and its outputs the rest of the prime. None on
    /// an exact ratio. (`Rated::delay_ns` states RX's worst case instead,
    /// which would overstate the round trip by up to a block.)
    pub fn reblock_share(&self) -> (u64, u64) {
        let a = self.prime() as u64;
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

    /// How long the bridge may wait for the DAW's answer, app samples
    /// after the start of the device block its DAW block runs in
    /// ([`Served`] sets that start, [`crate::bridge::WaitFrom`]): the
    /// device's offset (`device_offset`, its samples,
    /// [`crate::wait_offset`]) at the app's rate, rounded down, so the wait
    /// ends at the device block's own deadline. At 44.1 kHz a DAW block
    /// starts up to a block before that device block; the wait still
    /// counts from the device block (dasio#107).
    pub fn wait_offset(&self, device_offset: u64) -> u64 {
        self.at_app(device_offset)
    }

    /// The device's sample time `samples` on the app's clock, rounded
    /// down: the app samples that have passed by then.
    pub fn at_app(&self, samples: u64) -> u64 {
        (u128::from(samples) * u128::from(self.app_rate) / u128::from(self.device_rate.max(1)))
            as u64
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

/// The instance's processor for a [`Plan`]: the bridge alone at the
/// device's rate, behind the converters with a fixed app block at another.
pub enum Served<P> {
    Direct(P),
    Converted(Box<Converted<P>>),
}

/// The converted path: [`Rated`] with a fixed app block around the bridge,
/// and the bridge's [`WaitFrom`] set to each device block's start on the
/// app's clock before Rated runs that block.
pub struct Converted<P> {
    rated: Rated<P>,
    plan: Plan,
    wait_from: WaitFrom,
}

impl<P: Process> Converted<P> {
    /// One device block, the bridge's wait set to count from its start.
    fn run(&mut self, block: &mut Block<'_>) {
        // Rated hands the DAW a block that starts up to a block before
        // this device block (its RX FIFO's frames came first); the wait
        // counts from the device block's own start (dasio#107).
        self.wait_from
            .set(block.start().map(|s| self.plan.at_app(s)));
        self.rated.process(block);
    }
}

/// The counters of a converted path: the frames the converter's TX FIFO
/// played as silence.
#[derive(Debug, Clone, Default)]
pub struct Counters {
    pub converter_dry: Option<Arc<AtomicU64>>,
}

impl Counters {
    /// The converter's dry frames, 0 without a converter.
    pub fn read(&self) -> u64 {
        self.converter_dry
            .as_ref()
            .map_or(0, |a| a.load(Ordering::Relaxed))
    }
}

impl<P: Process> Served<P> {
    /// `inner` (the bridge, at the plan's app block) served by `plan` for
    /// `inputs` RX and `outputs` TX channels; the converter's refusal as
    /// text. On a converted path `wait_from` (the bridge's own,
    /// [`crate::bridge::Bridge::wait_from`]) is set to each device block's
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
        let rated = Rated::with_fixed_app_block(
            inner,
            plan.device_rate,
            plan.app_rate,
            plan.device_block as usize,
            inputs,
            outputs,
        )
        .map_err(|e| e.to_string())?;
        // dlink rounds the device block's period up the same way: the
        // DAW's buffer the driver announced.
        if rated.app_block() != Some(plan.app_block as usize) {
            return Err(format!(
                "the converter's app block {:?} is not the plan's {}",
                rated.app_block(),
                plan.app_block
            ));
        }
        let counters = Counters {
            converter_dry: Some(rated.dry()),
        };
        let converted = Converted {
            rated,
            plan: *plan,
            wait_from,
        };
        Ok((Served::Converted(Box::new(converted)), counters))
    }
}

impl<P: Process> Process for Served<P> {
    #[cfg_attr(dasio_rtsan, sanitize(realtime = "nonblocking"))]
    fn process(&mut self, block: &mut Block<'_>) {
        match self {
            Served::Direct(p) => p.process(block),
            Served::Converted(p) => p.run(block),
        }
    }

    fn discontinuity(&mut self) {
        match self {
            Served::Direct(p) => p.discontinuity(),
            Served::Converted(p) => p.rated.discontinuity(),
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
