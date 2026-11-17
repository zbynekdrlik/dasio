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
//! so a bridge instance checks the PC profile in its own process, without
//! the entries whose reading starts a process (dasio#120), never through
//! the DAW kind's `dasio pc-profile check` child.
//!
//! **The to-ASIO side is pulled** (dasio#120, dstack issue 307 step 2,
//! v0.6.155): the RX rings are the app's to write (claimed before the
//! pump, whose map carries no RX channel), and each buffer switch of the
//! driver drains the flow sockets and reads the rings by position into the
//! to-ASIO FIFO before it plays, so a late pump wake no longer runs that
//! FIFO dry. The run loop looks at the pull on its own 250 ms timer
//! ([`crate::to_asio_pull`]).
//!
//! **The from-ASIO side is pulled too** (dasio#120, dstack issue 364,
//! v0.6.167): the from-ASIO player leaves the pump's side for a
//! [`FromAsio`] the device's TX thread runs in whole blocks just before it
//! sends, so a late pump no longer sends silence from the driver. The pump
//! writes no channel at all; its side only takes the driver's handovers.
//!
//! **The ASRC starts from the last run's lock** (dasio#120, dstack issue
//! 382, v0.6.172): the instance folder's seed file ([`crate::asrc_seed`])
//! is read before the driver's first open and written from the run loop
//! when the bridge's seed changed (once a minute at most) and at the end.
//!
//! **Each underrun names its cause** (dasio#120, dstack issue 386,
//! v0.6.173): the counts tick hands the players' underruns and the driver
//! callbacks' lateness bins to dstack's [`UnderrunWatch`], which WARNs
//! when a count rose. The converter's length is the config's
//! (`[bridge] converter_taps`, dstack issue 382, v0.6.174); the status and
//! the counts line carry each direction's converter delay.
//!
//! **dcontroller reads these figures over the network** (dasio#120,
//! dstack issue 391, v0.6.176, dcontroller#108): once the runner runs, the
//! bridge becomes the source of the device's figure slot (targets,
//! converter delays and callback lateness, read at each request), and each
//! counts tick hands the slot the underruns and their causes right after
//! the watch's look. The device answers dstack's bridge-figures request on
//! the info port 8700; without the source it answers nothing.
//!
//! [`FromAsio`]: dstack_asio::bridge::FromAsio
//! [`DanteSide`]: dstack_asio::bridge::DanteSide
//! [`AsioSide`]: dstack_asio::bridge::AsioSide

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dlink::bridge::RxPull;
use dlink::bridge::pull::PullStats;
use dstack::host::{self, Signals, Stopped};
use dstack::media::tx::{TxFill, TxFillStats};
use dstack::stream::{self, Map, Pump, PumpStats, Timing};
use dstack_asio::bridge::{Bridge, Seed, UnderrunWatch};
use dstack_asio::win::WinDevice;
use dstack_asio::{Channels, MakeProcess, Opened, Process, Reason, Runner, Status};
use tracing::{error, info, warn};

use crate::app_status::BridgeFacts;
use crate::asrc_seed::{self, Keeper};
use crate::kind::BridgeConfig;
use crate::logfile::{COUNTS_EVERY, Every, STATUS_EVERY};
use crate::run::profile::ProfileWatch;
use crate::run::{StopEvent, place_helpers, start_device};

/// How long the start waits for the runner's thread to hand over its
/// status (it only makes the COM apartment and the runner first).
const RUNNER_START: Duration = Duration::from_secs(5);

/// The PC profile's first in-process check, after the start (dasio#120).
const PROFILE_FIRST: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bridge instance being run: its number and its folder (its
/// config's, where the ASRC seed file lives).
pub struct Instance<'a> {
    /// The instance number (`dasio-N`).
    pub number: u32,
    /// The instance's folder.
    pub dir: &'a Path,
}

/// Runs bridge instance `at` (its `[bridge]` table `bridge`, its device
/// config `cfg`) on `ip` with the media clock from `clock` until Ctrl+C,
/// the instance's stop event, `stop_asked` or the device's end.
pub fn run(
    at: Instance<'_>,
    bridge: BridgeConfig,
    cfg: dstack::device::DeviceConfig,
    ip: std::net::Ipv4Addr,
    clock: host::ClockSource,
    tuning: crate::Tuning,
    stop_asked: Arc<AtomicBool>,
) -> Result<(), String> {
    let instance = at.number;
    // The map against the device's channels before the device starts.
    let spec = bridge.spec(&cfg)?;
    let channels = bridge.channels();
    info!(
        instance,
        bridge = %bridge.describe(),
        "a bridge instance: no DAW, no shared memory, and no child process while the driver is loaded (the PC profile is checked in process, without bcd/task entries)"
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
    // The ASRC seed before the driver's first open (dstack v0.6.172).
    let seed_path = asrc_seed::path(at.dir, &bridge.driver);
    let mut keeper = seed_bridge(&mut b, &seed_path);
    // Before Pump::new, which would claim the RX rings itself.
    pull_to_driver(&mut b, &media, timing.playout, cfg.sample_rate)?;
    let pull_stats = b.to_asio_pull_stats();
    // Before the pump's side, the device's start and the driver's first
    // open (dstack v0.6.167): the TX thread fills the from-ASIO channels.
    let from_pull = pull_from_driver(&mut b, &media, engine_tx, timing, cfg.encoding)?;
    let dante = b
        .dante_side()
        .ok_or("the bridge's pump side was taken already")?;
    let (to_stats, from_stats, to_fifo) =
        (b.to_asio_stats(), b.from_asio_stats(), b.to_asio_fifo());
    // The from-ASIO figures (dstack #325, dasio#120).
    let age = b.from_asio_age();
    let shared = Arc::new(Mutex::new(b));
    // No channel for the pump: the driver's switch pulls the RX, the TX
    // thread fills the TX (one writer a ring).
    let map = Map {
        rx: vec![],
        tx: vec![],
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
    // The driver after the pump and the device run: the to-ASIO player's
    // handover needs the pump to take it, the from-ASIO one the TX thread.
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
    // From now on the device answers dcontroller's figures read (dstack issue 391).
    lock(&shared).publish_figures(media.bridge_figures(), Arc::clone(&status));
    // The PC profile's problems (dasio#5, #120); u32::MAX while there is none.
    let pc_problems = Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX));
    // The measured RX packet age at arrival (dasio#128): information
    // only, the latency stays the owner's decision.
    let mut rx_arrival = crate::rx_arrival::Watch::default();
    {
        let kernel_tx = tuning.kernel_tx;
        let rx_figures = rx_arrival.arrival();
        let (status, to_stats, from_stats, driver, pc_problems, age, from_pull) = (
            Arc::clone(&status),
            Arc::clone(&to_stats),
            Arc::clone(&from_stats),
            bridge.driver.clone(),
            Arc::clone(&pc_problems),
            Arc::clone(&age),
            from_pull.clone(),
        );
        media.set_app_status(move || {
            let s = lock(&status).clone();
            crate::app_status::bridge_json(&BridgeFacts {
                rx: rx_figures.report(),
                from_asio_pull: from_pull.as_ref().map(|p| p.view()),
                to_asio_converter_delay_ns: to_stats.converter_delay_ns(),
                from_asio_converter_delay_ns: from_stats.converter_delay_ns(),
                ..BridgeFacts::of(
                    &driver,
                    &s,
                    to_stats.underruns(),
                    from_stats.underruns(),
                    match pc_problems.load(Ordering::Relaxed) {
                        u32::MAX => None,
                        n => Some(n),
                    },
                    crate::run::tx_path(kernel_tx),
                    age.report(),
                )
            })
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
    // The look at the to-ASIO pull, on its own timer (dstack v0.6.155).
    let mut keep = Every::new(crate::to_asio_pull::KEEP_EVERY, start, false);
    // The look for a new ASRC seed to keep (dasio#120).
    let mut save = Every::new(asrc_seed::SAVE_EVERY, start, false);
    // The underrun counts at the last counts tick (dstack v0.6.173).
    let mut underruns = UnderrunWatch::new();
    let profile = ProfileWatch::new(own);
    // The first check a minute in: after the helper placer has moved this
    // process's threads off the media CPUs (a thread on a media CPU takes
    // the quantum a spin tail yields, dasio#1).
    let mut profile_due = start + PROFILE_FIRST;
    let why = loop {
        // The players the pump gave back, freed here, off its thread.
        lock(&shared).collect();
        let now = Instant::now();
        if keep.due(now) {
            look_at_pull(&shared, pull_stats.as_deref());
        }
        if save.due(now) {
            save_seed(&shared, &mut keeper, &seed_path);
        }
        if now >= profile_due {
            profile_due = now + Duration::from_secs(crate::pc_profile::CHECK_EVERY_S);
            profile.start_in_process(&pc_problems);
            if let Some(r) = rx_arrival.report() {
                crate::rx_arrival::log(&r);
            }
        }
        if counts.due(now) {
            if let Some(r) =
                rx_arrival.observe(crate::rx_arrival::flows(&media.rx), cfg.sample_rate)
            {
                crate::rx_arrival::log(&r);
            }
            // The TX packets' age and the receivers' late counts: here,
            // never in the pump or the driver's callback (the view locks the
            // TX table); the device's only reader of the view; dstack WARNs
            // at most once an hour.
            crate::app_status::from_asio_age_tick(
                &age,
                &media.tx.take_app_view(),
                media.rx.network_p99_ns(),
            );
            let s = lock(&status).clone();
            // A player's underruns that rose since the last tick, with the
            // driver callbacks' lateness meanwhile: dstack WARNs. The counts
            // and causes then go to dcontroller's read (dstack issue 391).
            crate::app_status::underrun_tick(
                &mut underruns,
                &s,
                to_stats.underruns(),
                from_stats.underruns(),
                from_stats.target_ms(),
            );
            underruns.publish(media.bridge_figures());
            let v = stats.view();
            info!(
                state = ?s.state,
                reason = s.reason.as_ref().map(ToString::to_string).unwrap_or_default(),
                rate = s.rate,
                buffer_frames = s.buffer_frames,
                callbacks = s.callbacks,
                late_callbacks = s.late_callbacks,
                // 0.25 ms bins under two periods (dstack v0.6.173).
                late_bins = ?s.late_bins,
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
                to_asio_converter_delay_ms = to_stats.converter_delay_ns() as f64 / 1e6,
                to_asio_overflows = to_fifo.as_ref().map_or(0, |f| f.overflows()),
                to_asio_pulls = pull_stats.as_ref().map_or(0, |p| p.pulls()),
                to_asio_pull_restarts = pull_stats.as_ref().map_or(0, |p| p.restarts()),
                to_asio_idle_drains = pull_stats.as_ref().map_or(0, |p| p.idle_drains()),
                from_asio_underruns = from_stats.underruns(),
                from_asio_ppm = from_stats.ppm(),
                from_asio_target_ms = from_stats.target_ms(),
                from_asio_converter_delay_ms = from_stats.converter_delay_ns() as f64 / 1e6,
                // None for a bridge with no channel from ASIO (no pull).
                from_asio_pull = ?from_pull.as_ref().map(|p| p.view()),
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
        // dasio's NDIS filter came unbound from the adapter: no silent
        // UDP fallback (dasio#125); the SCM restarts the device, whose
        // start checks the binding again.
        if tuning.kernel_tx && crate::kernel_tx::windows::unbound() {
            let e = "dasio's NDIS filter is no longer bound to the device's adapter: the device ends (dasio does not run without it)".to_string();
            error!(why = %e, "the device ends");
            break Some(e);
        }
        // A must-have item of the baseline went missing (dasio#128): the
        // device ends, the SCM restarts it and its start names it again.
        if tuning.kernel_tx
            && let Some(e) = profile.must_have()
        {
            error!(why = %e, "the device ends");
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
    // The last lock for the next start.
    save_seed(&shared, &mut keeper, &seed_path);
    match why {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// Starts `b`'s ASRC from the seed file at `path` (dasio#120, dstack
/// v0.6.172), before the driver's first open; the keeper of that file.
fn seed_bridge(b: &mut Bridge, path: &Path) -> Keeper {
    let start = asrc_seed::at_start(&Seed::read(path));
    if start.warn {
        warn!(path = %path.display(), "{}", start.line);
    } else {
        info!(path = %path.display(), "{}", start.line);
    }
    if let Some(s) = start.seed {
        b.seed_from(s);
    }
    Keeper::new(start.seed)
}

/// Writes the bridge's ASRC seed to `path` when it changed since the file
/// was read or last written (dasio#120): from the run loop, never the
/// driver's thread (a file write); a failed write is retried next time.
fn save_seed(shared: &Mutex<Bridge>, keeper: &mut Keeper, path: &Path) {
    let Some(s) = keeper.due(lock(shared).seed()) else {
        return;
    };
    let result = s.write(path);
    if result.is_ok() {
        info!(path = %path.display(), "ASRC seed kept for the next start: {}", asrc_seed::describe(&s));
    }
    if let Some(e) = keeper.wrote(s, result) {
        warn!(path = %path.display(), error = %e, "the ASRC seed could not be written: the next start may begin unseeded or from an older lock");
    }
}

/// Hands the to-ASIO side of `b` to the driver's buffer switch (dasio#120,
/// dstack issue 307 step 2): the RX rings' writes claimed for the app
/// (before `Pump::new`, which would claim them), an `RxPull` reading
/// to-ASIO channel `i` from RX channel `i` (`kind.rs`: one to-ASIO channel
/// per RX channel, in RX order) at the device's playout `playout`, handed
/// over before the driver's first open. A bridge with no to-ASIO channel
/// has no FIFO and nothing to pull.
fn pull_to_driver(
    b: &mut Bridge,
    media: &dstack::daemon::media::Media,
    playout: u64,
    rate: u32,
) -> Result<(), String> {
    let Some(fifo) = b.to_asio_fifo() else {
        return Ok(());
    };
    let intake = media
        .rx
        .app_intake()
        .ok_or("the to-ASIO pull: the device's RX rings have another writer already")?;
    let pull = RxPull::new(
        intake,
        Arc::clone(&media.clock),
        Arc::clone(&media.rx),
        (0..b.spec().to_asio).collect(),
        fifo,
        playout,
        rate,
    )
    .map_err(|e| format!("the to-ASIO pull: {e}"))?;
    b.pull_to_asio(pull)
        .map_err(|e| format!("the to-ASIO pull: {e}"))
}

/// Hands the from-ASIO side of `b` to the device's TX thread (dasio#120,
/// dstack issue 364, v0.6.167): the pull taken out of the pump's side
/// (before `dante_side` and the driver's first open) and claimed as the
/// TX fill of `engine_tx` (one TX channel per from-ASIO channel, in TX
/// order, `kind.rs`) at the pump's own `timing` and `bits`, before the
/// device's start builds the TX thread. A refusal is a start error: the
/// pull went with it, there is no pump path left to fall back to. A bridge
/// with no channel from ASIO has nothing to pull: `None`. Returns the
/// fill's counts (the status' `tx_pull`).
fn pull_from_driver(
    b: &mut Bridge,
    media: &dstack::daemon::media::Media,
    engine_tx: Vec<usize>,
    timing: Timing,
    bits: u8,
) -> Result<Option<Arc<TxFillStats>>, String> {
    if b.spec().from_asio == 0 {
        return Ok(None);
    }
    let from = b
        .pull_from_asio()
        .map_err(|e| format!("the from-ASIO pull: {e}"))?;
    let fill = TxFill::new(
        Box::new(from),
        engine_tx,
        timing,
        bits,
        Arc::clone(&media.rx),
    );
    let stats = Arc::clone(fill.stats());
    media
        .tx
        .claim_fill(fill)
        .map_err(|r| format!("the from-ASIO pull: {}", r.reason))?;
    Ok(Some(stats))
}

/// The run loop's look at the to-ASIO pull (dstack v0.6.155's
/// `Bridge::keep_pulled`; [`crate::to_asio_pull`]): the idle drain while
/// no driver side pulls, and its lines. The bridge's lock only for the
/// look itself, not the log lines.
fn look_at_pull(shared: &Mutex<Bridge>, stats: Option<&PullStats>) {
    let Some((look, poisoned)) = lock(shared).keep_pulled() else {
        return;
    };
    let (pulls, restarts, frames) =
        stats.map_or((0, 0, 0), |p| (p.pulls(), p.restarts(), p.frames()));
    for note in crate::to_asio_pull::notes(look, poisoned, pulls) {
        let rose = match note {
            crate::to_asio_pull::Note::Restarted(n) => n,
            _ => 0,
        };
        if note.warns() {
            warn!(pulls, restarts, rose, frames, "{}", note.text());
        } else {
            info!(pulls, restarts, frames, "{}", note.text());
        }
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
