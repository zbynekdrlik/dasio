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
    tuning: crate::Tuning,
    stop_asked: Arc<AtomicBool>,
) -> Result<(), String> {
    let spare = tuning.spare;
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
    if clock == host::ClockSource::System {
        warn!(
            ?clock,
            "test clock: PTP time is this machine's clock, not the network's"
        );
    }
    let prepared = host::prepare_with(&opts, &engine_tx, clock).map_err(|e| e.to_string())?;
    let cfg = prepared.config().clone();
    let media = prepared.media();
    if tuning.kernel_tx {
        kernel_tx(&media, ip);
    }
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
    )
    .with_deadline(crate::wait_offset(&timing, cfg.sample_rate), {
        // The pump's media clock, in samples (as the pump reads it).
        let clock = Arc::clone(&media.clock);
        let rate = cfg.sample_rate;
        move || clock.now_ns().map(|ns| dstack::clock::to_samples(ns, rate))
    });
    let stale = bridge.stale_counter();
    let waited = bridge.waited_counter();
    // dtray's line for this instance (dstack's app status): the header's
    // numbers, the bridge's counters, the late blocks of the last minute
    // (kept by the loop below).
    let late_per_min = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // The PC profile's problems (dasio#5); u32::MAX while there is none.
    let pc_problems = Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX));
    {
        let (shared, stale, waited, late_per_min, pc_problems) = (
            Arc::clone(&shared),
            Arc::clone(&stale),
            Arc::clone(&waited),
            Arc::clone(&late_per_min),
            Arc::clone(&pc_problems),
        );
        let (block, rate) = (shape.block, cfg.sample_rate);
        media.set_app_status(move || {
            let h = shared.header();
            crate::app_status::json(&crate::app_status::Facts {
                attached: h.host_attached.load(Ordering::Relaxed) != 0,
                block,
                sample_rate: rate,
                latency_in: latency.0,
                latency_out: latency.1,
                spare_blocks: spare,
                late_blocks: h.late.load(Ordering::Relaxed),
                driver_missed: h.missed.load(Ordering::Relaxed),
                stale: stale.load(Ordering::Relaxed),
                waited: waited.load(Ordering::Relaxed),
                heartbeat_ms: h.heartbeat_ms.load(Ordering::Relaxed),
                late_per_min: late_per_min.load(Ordering::Relaxed),
                pc_problems: match pc_problems.load(Ordering::Relaxed) {
                    u32::MAX => None,
                    n => Some(n),
                },
            })
        });
    }
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
        wait_offset = crate::wait_offset(&timing, cfg.sample_rate),
        "dasio instance serving"
    );
    let stop_event = StopEvent::create(instance).map_err(|e| format!("the stop event: {e}"))?;
    let start = Instant::now();
    let mut counts = Every::new(COUNTS_EVERY, start, false);
    let mut status = Every::new(STATUS_EVERY, start, true);
    let mut window = crate::app_status::LateWindow::default();
    let mut window_s = u64::MAX;
    let profile = ProfileWatch::new(own);
    if own != 0 {
        std::thread::Builder::new()
            .name("helper-placer".into())
            .spawn(move || place_helpers(own))
            .map_err(|e| format!("the helper placer: {e}"))?;
    }
    let mut profile_at: Option<Instant> = None;
    let why = loop {
        let h = shared.header();
        h.heartbeat_ms
            .store(start.elapsed().as_millis() as u64, Ordering::Release);
        let now = Instant::now();
        let secs = start.elapsed().as_secs();
        if secs != window_s {
            window_s = secs;
            late_per_min.store(
                window.take(secs, h.late.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
        if profile_at.is_none_or(|t| {
            now.duration_since(t) >= Duration::from_secs(crate::pc_profile::CHECK_EVERY_S)
        }) {
            profile_at = Some(now);
            profile.start(&pc_problems);
        }
        if counts.due(now) {
            let v = stats.view();
            info!(
                seq = h.seq.load(Ordering::Relaxed),
                ack = h.ack.load(Ordering::Relaxed),
                late = h.late.load(Ordering::Relaxed),
                driver_missed = h.missed.load(Ordering::Relaxed),
                stale = stale.load(Ordering::Relaxed),
                waited = waited.load(Ordering::Relaxed),
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

/// Hands dstack dasio's NDIS filter as the audio TX path (dasio#1); the
/// UDP sockets stay when the filter cannot be opened or dstack refuses.
fn kernel_tx(media: &dstack::daemon::media::Media, ip: std::net::Ipv4Addr) {
    match crate::kernel_tx::windows::KernelTx::open(ip) {
        Ok(k) => {
            let if_index = k.if_index();
            if media.set_frame_sender(Arc::new(k)) {
                info!(
                    if_index,
                    "audio TX goes through dasio's NDIS filter (kernel TX)"
                );
            } else {
                warn!(
                    if_index,
                    "audio TX stays on the UDP sockets: dstack refused the filter's frame sender"
                );
            }
        }
        Err(e) => {
            warn!(%e, "audio TX stays on the UDP sockets: dasio's NDIS filter could not be opened (is dasio_pacer installed?)")
        }
    }
}

/// Keeps every thread of this process but the media threads on the CPUs
/// those leave free ([`crate::helper_cpus`]), once a second: Process Lasso
/// resets thread pins when it sets the process's affinity, and threads
/// start later (the runtime, mDNS). The thread moves itself first, so its
/// own work never runs on a media CPU.
fn place_helpers(own: usize) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, GetProcessAffinityMask,
        GetThreadDescription, OpenThread, SetThreadAffinityMask, THREAD_QUERY_LIMITED_INFORMATION,
        THREAD_SET_INFORMATION,
    };
    // SAFETY: no arguments.
    let pid = unsafe { GetCurrentProcessId() };
    let mut last: Option<(usize, u32)> = None;
    loop {
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: this process's pseudo handle and two writable masks.
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) };
        let helpers = crate::helper_cpus(process, own);
        if helpers != 0 {
            // SAFETY: this thread's pseudo handle.
            unsafe { SetThreadAffinityMask(GetCurrentThread(), helpers) };
            if last.is_none_or(|(m, _)| m != helpers) {
                default_cpu_sets(helpers);
            }
            let mut moved = 0u32;
            // SAFETY: a thread snapshot, closed below.
            let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if snap != INVALID_HANDLE_VALUE {
                // SAFETY: a zeroed entry with its size set, as the API wants.
                let mut e: THREADENTRY32 = unsafe { std::mem::zeroed() };
                e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                // SAFETY: a valid snapshot and entry.
                let mut ok = unsafe { Thread32First(snap, &mut e) } != 0;
                while ok {
                    if e.th32OwnerProcessID == pid {
                        // SAFETY: a handle to one of this process's threads,
                        // closed below.
                        let h = unsafe {
                            OpenThread(
                                THREAD_SET_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION,
                                0,
                                e.th32ThreadID,
                            )
                        };
                        if !h.is_null() {
                            let mut text: *mut u16 = std::ptr::null_mut();
                            // SAFETY: a valid handle; Windows allocates the
                            // name, freed with LocalFree.
                            let name = if unsafe { GetThreadDescription(h, &mut text) } >= 0
                                && !text.is_null()
                            {
                                // SAFETY: a NUL-terminated string from Windows.
                                let n = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
                                // SAFETY: `n` chars before the NUL.
                                let s = String::from_utf16_lossy(unsafe {
                                    std::slice::from_raw_parts(text, n)
                                });
                                // SAFETY: allocated by GetThreadDescription.
                                unsafe { LocalFree(text.cast()) };
                                s
                            } else {
                                String::new()
                            };
                            // SAFETY: a valid handle with SET_INFORMATION.
                            if !crate::is_media_thread(&name)
                                && unsafe { SetThreadAffinityMask(h, helpers) } != 0
                            {
                                moved += 1;
                            }
                            // SAFETY: opened above.
                            unsafe { CloseHandle(h) };
                        }
                    }
                    // SAFETY: as above.
                    ok = unsafe { Thread32Next(snap, &mut e) } != 0;
                }
                // SAFETY: the snapshot opened above.
                unsafe { CloseHandle(snap) };
            }
            if last.is_none_or(|(m, n)| m != helpers || n != moved) {
                info!(
                    cpus = format!("{helpers:#x}"),
                    threads = moved,
                    "the helper threads kept off the media CPUs"
                );
                last = Some((helpers, moved));
            }
        } else if last != Some((0, 0)) {
            info!(
                process = format!("{process:#x}"),
                media = format!("{own:#x}"),
                "no CPU left for the helper threads: they stay on the process's CPUs"
            );
            last = Some((0, 0));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The process's default CPU sets: the helper CPUs, so a thread the
/// runtime starts later begins there, not on a media CPU, before the
/// placer's next pass (band, 5.11.2026). The media threads' affinity masks
/// take precedence over CPU sets.
fn default_cpu_sets(helpers: usize) {
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessDefaultCpuSets};
    let mut len = 0u32;
    // SAFETY: a size query (no buffer).
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, std::ptr::null_mut(), 0)
    };
    let mut buf = vec![0u8; len as usize];
    // SAFETY: a buffer of `len` bytes.
    if unsafe {
        GetSystemCpuSetInformation(
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0
    {
        warn!(err = %std::io::Error::last_os_error(), "the CPU sets could not be read");
        return;
    }
    let mut sets = Vec::new();
    let mut at = 0usize;
    while at + std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>() <= len as usize {
        // SAFETY: an entry Windows wrote at `at`; read unaligned.
        let e: SYSTEM_CPU_SET_INFORMATION =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
        if e.Size == 0 {
            break;
        }
        // SAFETY: entries of type CpuSetInformation (0), the only type.
        let c = unsafe { e.Anonymous.CpuSet };
        sets.push((c.Group, c.LogicalProcessorIndex, c.Id));
        at += e.Size as usize;
    }
    let ids = crate::cpu_set_ids(helpers, &sets);
    // SAFETY: this process's pseudo handle and `ids.len()` ids.
    if unsafe { SetProcessDefaultCpuSets(GetCurrentProcess(), ids.as_ptr(), ids.len() as u32) } == 0
    {
        warn!(err = %std::io::Error::last_os_error(), "the process's default CPU sets were not set");
    } else {
        info!(cpus = format!("{helpers:#x}"), ids = ?ids, "new threads start on the helper CPUs (default CPU sets)");
    }
}

/// The PC profile's checks (dasio#5): each problem is logged when it
/// appears and when it goes; the count is the tray's. The check runs in a
/// `dasio pc-profile check` child at normal priority on the CPUs outside
/// the media threads': on this process's own thread it held a CPU a media
/// thread's spin tail yielded, for a whole quantum (16 ms gaps at the
/// receivers every 10 min, FOH, dasio#1, 4.11.2026).
struct ProfileWatch {
    /// The media threads' CPUs, kept free of the child.
    own: usize,
    busy: Arc<AtomicBool>,
    last: Arc<Mutex<Option<Vec<String>>>>,
}

impl ProfileWatch {
    fn new(own: usize) -> Self {
        Self {
            own,
            busy: Arc::new(AtomicBool::new(false)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts a check (none while one runs); the count lands in `problems`
    /// (u32::MAX without a profile).
    fn start(&self, problems: &Arc<std::sync::atomic::AtomicU32>) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (own, busy, last, problems) = (
            self.own,
            Arc::clone(&self.busy),
            Arc::clone(&self.last),
            Arc::clone(problems),
        );
        let spawned = std::thread::Builder::new()
            .name("pc-profile".into())
            .spawn(move || {
                // No profile on this PC: no child to start.
                let path = std::path::Path::new(crate::pc_profile::DEFAULT_PATH);
                let found = if path.exists() {
                    child_check(own)
                } else {
                    Ok(crate::pc_profile::ChildCheck::NoProfile(
                        "the file does not exist".into(),
                    ))
                };
                let n = match found {
                    Ok(found) => record(&last, found),
                    Err(e) => {
                        warn!(%e, "the PC profile check did not run");
                        u32::MAX
                    }
                };
                problems.store(n, Ordering::Relaxed);
                busy.store(false, Ordering::Release);
            });
        if let Err(e) = spawned {
            warn!(%e, "the PC profile check's thread did not start");
            self.busy.store(false, Ordering::Release);
        }
    }
}

/// Logs what appeared and what went since the last check; the count.
fn record(last: &Mutex<Option<Vec<String>>>, found: crate::pc_profile::ChildCheck) -> u32 {
    use crate::pc_profile::ChildCheck;
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    let now = match found {
        ChildCheck::NoProfile(why) => {
            if last.is_none() {
                info!(path = crate::pc_profile::DEFAULT_PATH, %why, "no PC profile: its checks are off");
                *last = Some(Vec::new());
            }
            return u32::MAX;
        }
        ChildCheck::Problems(now) => now,
    };
    let before = last.take().unwrap_or_default();
    for t in now.iter().filter(|t| !before.contains(t)) {
        warn!("PC profile: {t}");
    }
    for t in before.iter().filter(|t| !now.contains(t)) {
        info!("PC profile: fixed: {t}");
    }
    if now.is_empty() {
        info!("PC profile checked: as it should be");
    }
    let n = now.len() as u32;
    *last = Some(now);
    n
}

/// Runs `dasio pc-profile check` (this exe) at normal priority, without a
/// window, moved off `own` (the media CPUs) at once, and reads its output.
fn child_check(own: usize) -> Result<crate::pc_profile::ChildCheck, String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessAffinityMask, SetProcessAffinityMask,
    };
    #[link(name = "ntdll")]
    unsafe extern "system" {
        /// Resumes every thread of a process (ntdll; no Win32 wrapper).
        fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
    }
    // The plain `dasio.exe` beside this instance's copy: Process Lasso's rule
    // for the instance's exe name would put the child back on the media
    // CPUs; `dasio.exe` falls under its catch-all.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plain = exe.with_file_name("dasio.exe");
    let exe = if plain.exists() { plain } else { exe };
    let child = std::process::Command::new(exe)
        .args(["pc-profile", "check"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(crate::child_creation_flags(own))
        .spawn()
        .map_err(|e| e.to_string())?;
    if own != 0 {
        // Started suspended: moved off the media CPUs first, then resumed.
        let (mut mine, mut system) = (0usize, 0usize);
        // SAFETY: this process's pseudo handle and two writable masks.
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut mine, &mut system) };
        let off = system & !own;
        if off != 0 {
            // SAFETY: the child's process handle, open while `child` lives.
            if unsafe { SetProcessAffinityMask(child.as_raw_handle(), off) } == 0 {
                warn!(err = %std::io::Error::last_os_error(), "the PC profile check stays on the media CPUs");
            }
        }
        // SAFETY: the child's process handle, suspended at its creation.
        let r = unsafe { NtResumeProcess(child.as_raw_handle()) };
        if r < 0 {
            let mut child = child;
            let _ = child.kill();
            return Err(format!(
                "the PC profile check did not resume (status {r:#x})"
            ));
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(crate::pc_profile::read_child_check(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    ))
}
