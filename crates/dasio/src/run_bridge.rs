//! `dasio run` for a bridge instance on Windows (dasio#106, the owner's
//! decision of 13.11.2026 on dstack#243): the instance's Dante device on
//! its own address, its pump running dstack-asio's [`DanteSide`], and a
//! local ASIO driver (e.g. Waves SoundGrid's virtual soundcard) opened as a
//! host by a [`Runner`] over [`WinDevice`] on its own thread, whose every
//! open builds the driver's [`AsioSide`] with [`Bridge::asio_side`]. The
//! clock crossing each way is dstack's (dlink::bridge's FIFO and ASRC
//! player at the app buffer, dstack#247).
//!
//! Thin glue: the config's decisions are [`crate::kind`], the tray's line
//! [`crate::app_status::bridge_json`]. No shared memory and no DAW: the
//! instance's own ASIO driver has nothing to attach to.
//!
//! **No child process while the driver is loaded:** a child spawned from a
//! process holding the SoundGrid driver makes it resync (songplayer #233),
//! so a bridge instance never starts the PC profile check (the DAW kind's
//! `dasio pc-profile check` child).
//!
//! [`DanteSide`]: dstack_asio::bridge::DanteSide
//! [`AsioSide`]: dstack_asio::bridge::AsioSide

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dstack::host::{self, Signals, Stopped};
use dstack::stream::{self, Map, Pump, PumpStats};
use dstack_asio::bridge::Bridge;
use dstack_asio::win::WinDevice;
use dstack_asio::{Channels, MakeProcess, Opened, Process, Reason, Runner, Status};
use tracing::{error, info, warn};

use crate::app_status::BridgeFacts;
use crate::kind::BridgeConfig;
use crate::logfile::{COUNTS_EVERY, Every, STATUS_EVERY};
use crate::run::{StopEvent, place_helpers, start_device};

/// How long the start waits for the runner's thread to hand over its
/// status (it only makes the COM apartment and the runner first).
const RUNNER_START: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs bridge instance `instance` (its `[bridge]` table `bridge`, its
/// device config `cfg`) on `ip` with the media clock from `clock` until
/// Ctrl+C, the instance's stop event, `stop_asked` or the device's end.
pub fn run(
    instance: u32,
    bridge: BridgeConfig,
    cfg: dstack::device::DeviceConfig,
    ip: std::net::Ipv4Addr,
    clock: host::ClockSource,
    tuning: crate::Tuning,
    stop_asked: Arc<AtomicBool>,
) -> Result<(), String> {
    // The map against the device's channels before the device starts.
    let spec = bridge.spec(&cfg)?;
    let channels = bridge.channels();
    info!(
        instance,
        bridge = %bridge.describe(),
        "a bridge instance: no DAW, no shared memory, and no child process while the driver is loaded (no PC profile check)"
    );
    let own = crate::own_cpus(&cfg);
    // What can fail without a running device first: past the device's
    // start every error path stops the runner, the pump and the device.
    let stop_event = StopEvent::create(instance).map_err(|e| format!("the stop event: {e}"))?;
    let (prepared, media, engine_tx) = start_device(cfg, ip, clock, tuning)?;
    let cfg = prepared.config().clone();
    let timing = crate::timing(&cfg);
    let stats = Arc::new(PumpStats::new(timing, cfg.sample_rate));
    media.attach_engine(Arc::clone(&stats));
    let mut b = Bridge::new(spec).map_err(|e| format!("the bridge: {e}"))?;
    let dante = b
        .dante_side()
        .ok_or("the bridge's pump side was taken already")?;
    let (to_stats, from_stats, to_fifo) =
        (b.to_asio_stats(), b.from_asio_stats(), b.to_asio_fifo());
    let shared = Arc::new(Mutex::new(b));
    let map = Map {
        rx: (0..cfg.rx.len()).collect(),
        tx: engine_tx,
    };
    let pump = Pump::new(
        dante,
        map,
        timing,
        cfg.encoding,
        Arc::clone(&media.rx),
        Arc::clone(&media.tx),
        Arc::clone(&stats),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let pump = stream::spawn(Arc::clone(&media.clock), pump, Arc::clone(&stop))
        .map_err(|e| format!("the pump: {e}"))?;
    let asked = Arc::new(AtomicBool::new(false));
    let device = prepared
        .start(Signals::Report(Arc::clone(&asked)))
        .map_err(|e| e.to_string())?;
    // The driver after the pump runs: the from-ASIO player's handover
    // needs the pump to take it.
    let runner_stop = Arc::new(AtomicBool::new(false));
    let (runner, status) = match spawn_runner(
        bridge.driver.clone(),
        channels,
        Arc::clone(&shared),
        Arc::clone(&runner_stop),
    ) {
        Ok(started) => started,
        Err(e) => {
            stop_device(&stop, pump, device);
            return Err(e);
        }
    };
    {
        let (status, to_stats, from_stats, driver) = (
            Arc::clone(&status),
            Arc::clone(&to_stats),
            Arc::clone(&from_stats),
            bridge.driver.clone(),
        );
        media.set_app_status(move || {
            let s = lock(&status).clone();
            crate::app_status::bridge_json(&BridgeFacts::of(
                &driver,
                &s,
                to_stats.underruns(),
                from_stats.underruns(),
            ))
        });
    }
    info!(
        instance,
        rx = cfg.rx.len(),
        tx = cfg.tx.len(),
        block = cfg.block,
        driver = %bridge.driver,
        "dasio bridge instance serving"
    );
    // The helper CPUs for every thread but the media threads: the
    // driver's own threads too (its buffer switch, e.g. SoundGrid's), as
    // the DAW's are in the DAW kind. Without the placer the threads stay
    // where they are: a warning, not the device's end.
    if own != 0
        && let Err(e) = std::thread::Builder::new()
            .name("helper-placer".into())
            .spawn(move || place_helpers(own))
    {
        warn!(%e, "the helper placer did not start: the helper threads stay where they are");
    }
    let start = Instant::now();
    let mut counts = Every::new(COUNTS_EVERY, start, false);
    let mut status_due = Every::new(STATUS_EVERY, start, true);
    let why = loop {
        // The players the pump gave back, freed here, off its thread.
        lock(&shared).collect();
        let now = Instant::now();
        if counts.due(now) {
            let s = lock(&status).clone();
            let v = stats.view();
            info!(
                state = ?s.state,
                reason = s.reason.as_ref().map(ToString::to_string).unwrap_or_default(),
                rate = s.rate,
                buffer_frames = s.buffer_frames,
                callbacks = s.callbacks,
                late_callbacks = s.late_callbacks,
                fast_callbacks = s.fast_callbacks,
                overloads = s.overloads,
                faults = s.faults,
                gap_max_ms = s.gap_max_ms,
                resets = s.resets,
                clock_waits = s.clock_waits,
                resyncs = s.resyncs,
                to_asio_underruns = to_stats.underruns(),
                to_asio_ppm = to_stats.ppm(),
                to_asio_target_ms = to_stats.target_ms(),
                to_asio_overflows = to_fifo.as_ref().map_or(0, |f| f.overflows()),
                from_asio_underruns = from_stats.underruns(),
                from_asio_ppm = from_stats.ppm(),
                from_asio_target_ms = from_stats.target_ms(),
                blocks = v.blocks,
                pump_late = v.late,
                missed = v.missed,
                overruns = v.overruns,
                max_process_us = v.max_process_us,
                "bridge counts"
            );
            if status_due.due(now) {
                info!(status = %dstack::ctl::answer("status", &media), "device status");
            }
        }
        if asked.load(Ordering::Acquire)
            || stop_event.is_set()
            || stop_asked.load(Ordering::Acquire)
        {
            info!("a stop was asked");
            break None;
        }
        if let Some(e) = device.ended() {
            break Some(e);
        }
        if let Some(e) = crate::health::pump_failure(&stats.stream_stats(), pump.is_finished()) {
            error!(why = %e, "the device ends");
            break Some(e);
        }
        // The runner loops until it is told to stop: a thread that ended
        // panicked, and the driver is no longer served.
        if lock(&status).reason == Some(Reason::Parked) {
            let e = "the ASIO driver is parked (a callback never returned; no open succeeds in this process): the device ends for the SCM to restart it".to_string();
            error!(why = %e, "the device ends");
            break Some(e);
        }
        if runner.is_finished() {
            let e = "the ASIO driver's runner thread ended (panicked): the device ends for the SCM to restart it".to_string();
            error!(why = %e, "the device ends");
            break Some(e);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // The driver first (its side stops feeding), then the pump and the
    // device.
    runner_stop.store(true, Ordering::Release);
    if runner.join().is_err() {
        warn!("the ASIO driver's runner thread panicked");
    }
    stop_device(&stop, pump, device);
    match why {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// Stops the pump (`stop`, then its thread) and the device.
fn stop_device(stop: &AtomicBool, pump: JoinHandle<()>, device: host::Host) {
    stop.store(true, Ordering::Release);
    if pump.join().is_err() {
        warn!("the pump's thread panicked");
    }
    match device.stop(Duration::from_secs(7)) {
        Stopped::Done => {}
        other => warn!(?other, "the device's stop"),
    }
}

/// The runner's thread `asio-bridge`: a [`WinDevice`] made there (its COM
/// STA lives on that thread), a [`Runner`] of `driver` with `channels`
/// whose every open builds the driver's side from `bridge`, looping until
/// `stop`. Returns the thread and the runner's status.
fn spawn_runner(
    driver: String,
    channels: Channels,
    bridge: Arc<Mutex<Bridge>>,
    stop: Arc<AtomicBool>,
) -> Result<(JoinHandle<()>, Arc<Mutex<Status>>), String> {
    let (tx, rx) = mpsc::channel();
    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("asio-bridge".into())
        .spawn(move || {
            let make: MakeProcess = Box::new(move |opened: &Opened| {
                let side = lock(&bridge).asio_side(opened)?;
                Ok(Box::new(side) as Box<dyn Process>)
            });
            let runner = Runner::new(WinDevice::new(), &driver, channels, make);
            // The start gave up waiting: no runner (rx is gone).
            if tx.send(runner.status()).is_err() {
                return;
            }
            runner.run(&thread_stop);
        })
        .map_err(|e| format!("the ASIO driver's runner thread: {e}"))?;
    match rx.recv_timeout(RUNNER_START) {
        Ok(status) => Ok((thread, status)),
        Err(e) => {
            // A runner that comes late stops at once: it holds no driver
            // before its first step.
            stop.store(true, Ordering::Release);
            Err(format!("the ASIO driver's runner did not start: {e}"))
        }
    }
}
