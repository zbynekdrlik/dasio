//! `dasio run` on Windows: the instance's Dante device on its own
//! address, its pump on the bridge, the shared memory for the driver, and a
//! heartbeat in the shared header until Ctrl+C or the device ends.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dstack::daemon::{Options, Place};
use dstack::host::{self, Signals, Stopped};
use dstack::media::source::Source;
use dstack::stream::{self, Map, Pump, PumpStats};
use shm::{Shape, Shared};
use tracing::{info, warn};

use crate::bridge::{Bridge, SharedLink};
use crate::logfile::{COUNTS_EVERY, Every, STATUS_EVERY};
use crate::turnaround::{Stamps, Turnaround};

/// Runs instance `instance` from the device config at `config` on `ip`
/// with the media clock from `clock` (dantesync in production; the system
/// clock only for a test machine without one) until Ctrl+C, the instance's
/// stop event, `stop_asked` (the service's Stop) or the device's end.
pub fn run(
    instance: u32,
    config: PathBuf,
    ip: std::net::Ipv4Addr,
    clock: host::ClockSource,
    spare: u32,
    stop_asked: Arc<AtomicBool>,
) -> Result<(), String> {
    let cfg = host::read_config(&config).map_err(|e| e.to_string())?;
    // The process on its own cores first: it starts with its parent's set
    // (on the FOH PC Process Lasso moves it later), and a thread cannot be
    // pinned outside its process's set (error 87 there).
    let own = crate::own_cpus(&cfg);
    if own != 0 {
        // The media threads pin to `own`; the process holds the whole L3
        // groups (CCDs) of those CPUs, for its other threads (the device's
        // runtime, mDNS, the log): confined to `own` they could not run
        // beside two spinning priority-31 threads, and a lock one of them
        // held kept the TX thread waiting 2.5 to 15 ms (FOH PC,
        // 2026-10-29); on every CPU, Process Lasso took the process for
        // its own rule and moved the media threads off their CPUs
        // (2026-11-03). Within the CCD they stay on the media threads'
        // chiplet.
        let mask = crate::process_cpus(own, &l3_groups(), started_cpus());
        crate::probe::set_affinity(mask)?;
        info!(mask = format!("{mask:#x}"), "the process's CPUs");
        // Its threads alone on those CPUs: the process at REALTIME, so its
        // media threads run at 31 (dstack's ask_realtime on Windows).
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
    if clock != host::ClockSource::Dsync {
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
    let latency = crate::latencies(&cfg, spare);
    let shared = Arc::new(
        Shared::create(instance, shape, cfg.sample_rate, latency)
            .map_err(|e| format!("the shared memory: {e}"))?,
    );
    let stats = Arc::new(PumpStats::new(timing, cfg.sample_rate));
    media.attach_engine(Arc::clone(&stats));
    let stamps = Arc::new(Stamps::default());
    let base = Instant::now();
    let bridge = Bridge::with_spare(
        SharedLink {
            shared: Arc::clone(&shared),
            stamps: Arc::clone(&stamps),
            base,
        },
        u64::from(spare),
    );
    let stale = bridge.stale_counter();
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
    let turnaround = Arc::new(Mutex::new(Turnaround::default()));
    let period_ns = u64::from(shape.block) * 1_000_000_000 / u64::from(cfg.sample_rate);
    let watcher = watch_turnaround(
        Arc::clone(&shared),
        stamps,
        base,
        period_ns,
        Arc::clone(&turnaround),
        Arc::clone(&stop),
    )
    .map_err(|e| format!("the turnaround watcher: {e}"))?;
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
        spare_blocks = spare,
        "dasio instance serving"
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
                stale = stale.load(Ordering::Relaxed),
                attached = h.host_attached.load(Ordering::Relaxed),
                blocks = v.blocks,
                pump_late = v.late,
                missed = v.missed,
                overruns = v.overruns,
                rebootstraps = v.rebootstraps,
                max_process_us = v.max_process_us,
                "counts"
            );
            // The DAW's turnaround since the last counts (see turnaround).
            if let Ok(t) = turnaround.lock().map(|mut t| t.take())
                && t.count > 0
            {
                info!(
                    blocks = t.count,
                    p50_us = t.p50_us,
                    p99_us = t.p99_us,
                    max_us = t.max_us,
                    over_period = t.over,
                    "DAW turnaround"
                );
            }
            if status.due(now) {
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
        std::thread::sleep(Duration::from_millis(100));
    };
    stop.store(true, Ordering::Release);
    let _ = pump.join();
    let _ = watcher.join();
    match device.stop(Duration::from_secs(7)) {
        Stopped::Done => {}
        other => warn!(?other, "the device's stop"),
    }
    match why {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// The turnaround watcher: sleeps on the driver's ready event, and each
/// newly returned block's time since its hand-over goes to `out`. A
/// sleeping thread at the top priority wakes within µs of the driver's
/// signal, so it measures the DAW, not itself, and spins no core.
fn watch_turnaround(
    shared: Arc<Shared>,
    stamps: Arc<Stamps>,
    base: Instant,
    period_ns: u64,
    out: Arc<Mutex<Turnaround>>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("turnaround".into())
        .spawn(move || {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
            };
            // SAFETY: the calling thread's pseudo handle.
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
            let mut last = 0u64;
            while !stop.load(Ordering::Acquire) {
                if !shared.wait_ready(100) {
                    continue;
                }
                let now = base.elapsed().as_nanos() as u64;
                let ack = shared.header().ack.load(Ordering::Acquire);
                if ack == last {
                    continue;
                }
                last = ack;
                if let Some(at) = stamps.published(ack)
                    && let Ok(mut t) = out.lock()
                {
                    t.record(now.saturating_sub(at), period_ns);
                }
            }
        })
}

/// `Global\dasio-<n>-stop`: `dasio stop` sets it from any
/// session (an install over ssh too), the running instance ends as on
/// Ctrl+C (never killed). The instance runs elevated (its logon task), so
/// it may create a global object.
pub struct StopEvent(windows_sys::Win32::Foundation::HANDLE);

fn stop_name(instance: u32) -> Vec<u16> {
    format!("Global\\dasio-{instance}-stop")
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

/// Each L3 cache's CPUs (processor group 0), e.g. one mask per AMD CCD;
/// empty when Windows does not say.
fn l3_groups() -> Vec<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationCache, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // SAFETY: a size query with no buffer.
    unsafe { GetLogicalProcessorInformationEx(RelationCache, std::ptr::null_mut(), &mut len) };
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` holds `len` writable bytes.
    let ok = unsafe {
        GetLogicalProcessorInformationEx(
            RelationCache,
            buf.as_mut_ptr()
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(),
            &mut len,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    // Records: Relationship (u32), Size (u32), then CACHE_RELATIONSHIP:
    // Level (u8) at 8, GroupCount (u16) at 38, GROUP_AFFINITY at 40
    // (Mask usize, Group u16).
    let mut groups = Vec::new();
    let mut at = 0usize;
    let bytes = &buf[..len as usize];
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap_or([0; 4])) as usize;
        if size == 0 || at + size > bytes.len() {
            break;
        }
        if size >= 50 && bytes[at + 8] == 3 {
            let mask = usize::from_le_bytes(bytes[at + 40..at + 48].try_into().unwrap_or([0; 8]));
            let group = u16::from_le_bytes([bytes[at + 48], bytes[at + 49]]);
            if group == 0 && mask != 0 {
                groups.push(mask);
            }
        }
        at += size;
    }
    groups
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
