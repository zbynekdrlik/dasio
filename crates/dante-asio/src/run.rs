//! `dante-asio run` on Windows: the instance's Dante device on its own
//! address, its pump on the bridge, the shared memory for the driver, and a
//! heartbeat in the shared header until Ctrl+C or the device ends.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iem_dante::daemon::{Options, Place};
use iem_dante::host::{self, Signals, Stopped};
use iem_dante::media::source::Source;
use iem_dante::stream::{self, Map, Pump, PumpStats};
use shm::{Shape, Shared};
use tracing::{info, warn};

use crate::bridge::{Bridge, SharedLink};
use crate::logfile::{COUNTS_EVERY, Every, STATUS_EVERY};

/// Runs instance `instance` from the device config at `config` on `ip`
/// with the media clock from `clock` (dantesync in production; the system
/// clock only for a test machine without one) until Ctrl+C, the instance's
/// stop event, `stop_asked` (the service's Stop) or the device's end.
pub fn run(
    instance: u32,
    config: PathBuf,
    ip: std::net::Ipv4Addr,
    clock: host::ClockSource,
    stop_asked: Arc<AtomicBool>,
) -> Result<(), String> {
    let cfg = host::read_config(&config).map_err(|e| e.to_string())?;
    // The process on its own cores first: it starts with its parent's set
    // (on the FOH PC Process Lasso moves it later), and a thread cannot be
    // pinned outside its process's set (error 87 there).
    let own = crate::own_cpus(&cfg);
    if own != 0 {
        // The media threads pin to `own`; the process keeps the CPUs it
        // started with too, for its other threads (the device's runtime,
        // mDNS, the log): confined to `own` they could not run beside two
        // spinning priority-31 threads, and a lock one of them held kept
        // the TX thread waiting 2.5 to 15 ms (FOH PC, 2026-10-29).
        let mask = started_cpus() | own;
        crate::probe::set_affinity(mask)?;
        info!(mask = format!("{mask:#x}"), "the process's CPUs");
        // Its threads alone on those CPUs: the process at REALTIME, so its
        // media threads run at 31 (iem_dante's ask_realtime on Windows).
        realtime_class()?;
    }
    let shape = Shape::new(cfg.rx.len() as u32, cfg.tx.len() as u32, cfg.block as u32)
        .ok_or("the channels (at most 64 each way) or the block do not fit the driver")?;
    let opts = Options {
        config,
        place: Place::Ip(ip),
        control: PathBuf::new(),
    };
    let engine_tx: Vec<usize> = (0..cfg.tx.len()).collect();
    if clock != host::ClockSource::Dantesync {
        warn!(
            ?clock,
            "test clock: PTP time is this machine's clock, not the network's"
        );
    }
    let prepared = host::prepare_with(&opts, &engine_tx, clock).map_err(|e| e.to_string())?;
    let cfg = prepared.config().clone();
    let media = prepared.media();
    for &tx in &engine_tx {
        media.tx.set_source(tx, Source::Engine);
    }
    let timing = crate::timing(&cfg);
    let latency = crate::latencies(&cfg);
    let shared = Arc::new(
        Shared::create(instance, shape, cfg.sample_rate, latency)
            .map_err(|e| format!("the shared memory: {e}"))?,
    );
    let stats = Arc::new(PumpStats::new(timing, cfg.sample_rate));
    media.attach_engine(Arc::clone(&stats));
    let bridge = Bridge::new(SharedLink(Arc::clone(&shared)));
    let map = Map {
        rx: (0..cfg.rx.len()).collect(),
        tx: engine_tx,
    };
    let pump = Pump::new(
        bridge,
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
    info!(
        instance,
        rx = shape.rx,
        tx = shape.tx,
        block = shape.block,
        latency_in = latency.0,
        latency_out = latency.1,
        "dante-asio instance serving"
    );
    let stop_event = StopEvent::create(instance).map_err(|e| format!("the stop event: {e}"))?;
    let start = Instant::now();
    let mut counts = Every::new(COUNTS_EVERY, start, false);
    let mut status = Every::new(STATUS_EVERY, start, true);
    let why = loop {
        let h = shared.header();
        h.heartbeat_ms
            .store(start.elapsed().as_millis() as u64, Ordering::Release);
        let now = Instant::now();
        if counts.due(now) {
            let v = stats.view();
            info!(
                seq = h.seq.load(Ordering::Relaxed),
                ack = h.ack.load(Ordering::Relaxed),
                late = h.late.load(Ordering::Relaxed),
                driver_missed = h.missed.load(Ordering::Relaxed),
                attached = h.host_attached.load(Ordering::Relaxed),
                blocks = v.blocks,
                pump_late = v.late,
                missed = v.missed,
                overruns = v.overruns,
                rebootstraps = v.rebootstraps,
                max_process_us = v.max_process_us,
                "counts"
            );
            if status.due(now) {
                info!(status = %iem_dante::ctl::answer("status", &media), "device status");
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
        std::thread::sleep(Duration::from_millis(100));
    };
    stop.store(true, Ordering::Release);
    let _ = pump.join();
    match device.stop(Duration::from_secs(7)) {
        Stopped::Done => {}
        other => warn!(?other, "the device's stop"),
    }
    match why {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// `Global\dante-asio-<n>-stop`: `dante-asio stop` sets it from any
/// session (an install over ssh too), the running instance ends as on
/// Ctrl+C (never killed). The instance runs elevated (its logon task), so
/// it may create a global object.
pub struct StopEvent(windows_sys::Win32::Foundation::HANDLE);

fn stop_name(instance: u32) -> Vec<u16> {
    format!("Global\\dante-asio-{instance}-stop")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

impl StopEvent {
    /// The running instance's event (manual reset, not set).
    pub fn create(instance: u32) -> std::io::Result<StopEvent> {
        use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};
        // SAFETY: a valid NUL-terminated name.
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, stop_name(instance).as_ptr()) };
        if h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the event just opened (a leftover set one is cleared).
        unsafe { ResetEvent(h) };
        Ok(StopEvent(h))
    }

    fn is_set(&self) -> bool {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        // SAFETY: a valid event handle.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for StopEvent {
    fn drop(&mut self) {
        // SAFETY: the handle this struct owns.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Asks instance `instance` to stop; `NotFound` when none runs.
pub fn ask_stop(instance: u32) -> std::io::Result<()> {
    use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    // SAFETY: a valid name.
    let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, stop_name(instance).as_ptr()) };
    if h.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let _owned = StopEvent(h);
    // SAFETY: a valid event handle.
    if unsafe { SetEvent(h) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// This process at REALTIME_PRIORITY_CLASS (elevated: the logon task's
/// highest privileges; otherwise Windows gives HIGH, which is reported).
fn realtime_class() -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, REALTIME_PRIORITY_CLASS, SetPriorityClass,
    };
    // SAFETY: the pseudo handle of this process.
    let set = unsafe { SetPriorityClass(GetCurrentProcess(), REALTIME_PRIORITY_CLASS) };
    // SAFETY: as above.
    let got = unsafe { GetPriorityClass(GetCurrentProcess()) };
    if set == 0 || got != REALTIME_PRIORITY_CLASS {
        warn!(
            class = format!("{got:#x}"),
            "the process is not at REALTIME"
        );
    } else {
        info!("the process at REALTIME");
    }
    Ok(())
}

/// The CPUs this process started with (its parent's set).
fn started_cpus() -> usize {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
    let (mut process, mut system) = (0usize, 0usize);
    // SAFETY: this process's pseudo handle and two writable masks.
    if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } == 0 {
        return 0;
    }
    process
}
