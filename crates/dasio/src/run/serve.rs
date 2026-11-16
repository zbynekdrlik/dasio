//! One serving of an instance's shared memory at the app's rate (dasio#107):
//! its section, the bridge (behind the rate conversion when the app's rate
//! is not the device's), the pump that runs it and the turnaround watcher.
//! The Dante device outlives a serving: a rate the DAW asks ends the
//! serving and starts the next one on the running device.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use dstack::device::DeviceConfig;
use dstack::stream::{self, Map, Pump, PumpStats, Timing};
use shm::{Shape, Shared};
use tracing::{info, warn};

use crate::app_rate::{Counters, Plan, Served, conversion_ns};
use crate::bridge::{Bridge, SharedLink};
use crate::turnaround::{Stamps, Turnaround};

/// What every serving of one run shares: the device and its pump's setup.
pub(super) struct Base<'a> {
    pub instance: u32,
    /// The device's shape (its channels and the pump's block).
    pub shape: Shape,
    pub cfg: &'a DeviceConfig,
    pub timing: Timing,
    pub spare: u32,
    pub media: &'a Arc<dstack::daemon::media::Media>,
    pub engine_tx: &'a [usize],
    pub stats: &'a Arc<PumpStats>,
    pub turnaround: &'a Arc<Mutex<Turnaround>>,
}

/// What dtray's line and the run's loop read of the serving now.
#[derive(Clone)]
pub(super) struct Current {
    pub shared: Arc<Shared>,
    pub plan: Plan,
    /// The DAW's input and output latency, app samples.
    pub latency: (u32, u32),
    pub stale: Arc<AtomicU64>,
    pub waited: Arc<AtomicU64>,
}

/// A running serving.
pub(super) struct Serving {
    pub current: Current,
    pub counters: Counters,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    watcher: Option<JoinHandle<()>>,
}

impl Serving {
    /// The pump's thread ended (it should run until [`Serving::end`]).
    pub fn pump_finished(&self) -> bool {
        self.pump.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stops the pump and the watcher and waits for them; again is a no-op.
    pub fn end(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(p) = self.pump.take()
            && p.join().is_err()
        {
            warn!("the pump's thread panicked");
        }
        if let Some(w) = self.watcher.take()
            && w.join().is_err()
        {
            warn!("the turnaround watcher's thread panicked");
        }
    }
}

impl Base<'_> {
    /// Serves the shared memory at `plan`'s rate: a section of the app's
    /// block (a new session when its shape is another), the bridge behind
    /// the conversion, and the pump and the turnaround watcher started.
    pub fn serve(&self, plan: Plan) -> Result<Serving, String> {
        let cfg = self.cfg;
        let device_latency = crate::latencies(cfg, self.spare);
        let conversion = conversion_ns(&plan)?;
        let latency = plan.latencies(device_latency, conversion);
        let shape = Shape::app(self.shape.rx, self.shape.tx, plan.app_block)
            .ok_or("the app's block does not fit the shared memory")?;
        let shared = Arc::new(
            Shared::create(self.instance, shape, plan.app_rate, latency)
                .map_err(|e| format!("the shared memory: {e}"))?,
        );
        let stamps = Arc::new(Stamps::default());
        let device_offset = crate::wait_offset(&self.timing, cfg.sample_rate);
        let offset = plan.wait_offset(device_offset);
        let bridge = Bridge::with_spare(
            SharedLink {
                shared: Arc::clone(&shared),
                stamps: Arc::clone(&stamps),
            },
            u64::from(self.spare),
        )
        .with_deadline(offset, {
            // The pump's media clock, in samples at the app's rate (the
            // bridge's blocks start in app samples).
            let clock = Arc::clone(&self.media.clock);
            let rate = plan.app_rate;
            move || clock.now_ns().map(|ns| dstack::clock::to_samples(ns, rate))
        })
        // A late block fades out and back in instead of cutting to silence
        // (dasio#1, the 8.11.2026 service clicks).
        .with_concealment(self.engine_tx.len(), shape.block as usize);
        let stale = bridge.stale_counter();
        let waited = bridge.waited_counter();
        let (processor, counters) =
            Served::new(bridge, &plan, self.shape.rx as usize, self.engine_tx.len())?;
        let map = Map {
            rx: (0..cfg.rx.len()).collect(),
            tx: self.engine_tx.to_vec(),
        };
        let pump = Pump::new(
            processor,
            map,
            self.timing,
            cfg.encoding,
            Arc::clone(&self.media.rx),
            Arc::clone(&self.media.tx),
            Arc::clone(self.stats),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = super::watch_turnaround(
            Arc::clone(&shared),
            stamps,
            plan.period_ns(),
            Arc::clone(self.turnaround),
            Arc::clone(&stop),
        )
        .map_err(|e| format!("the turnaround watcher: {e}"))?;
        let pump = match stream::spawn(Arc::clone(&self.media.clock), pump, Arc::clone(&stop)) {
            Ok(p) => p,
            Err(e) => {
                stop.store(true, Ordering::Release);
                let _ = watcher.join();
                return Err(format!("the pump: {e}"));
            }
        };
        info!(
            instance = self.instance,
            rx = shape.rx,
            tx = shape.tx,
            block = shape.block,
            device_block = plan.device_block,
            sample_rate = plan.app_rate,
            device_rate = plan.device_rate,
            converts = plan.converts(),
            exact = plan.exact,
            conversion_rx_ns = conversion.0,
            conversion_tx_ns = conversion.1,
            latency_in = latency.0,
            latency_out = latency.1,
            spare_blocks = self.spare,
            wait_offset = offset,
            session = shared.session(),
            "dasio instance serving"
        );
        Ok(Serving {
            current: Current {
                shared,
                plan,
                latency,
                stale,
                waited,
            },
            counters,
            stop,
            pump: Some(pump),
            watcher: Some(watcher),
        })
    }
}
