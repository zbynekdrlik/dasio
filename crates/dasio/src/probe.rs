//! `dasio probe` (plan Task 10): an ASIO host on the instance's driver
//! that measures the round trip through Dante. TX 1 sends one impulse a
//! second (one sample at half scale, silence else); RX 1 (subscribed to our
//! own TX 1 for the test) is scanned for it; each arrival's distance from
//! its emit is a round trip in samples. Inputs 2.. are passed to outputs
//! 2.. unchanged. The driver is reached as a DAW reaches it (its COM entry,
//! the CLSID as the interface), in this process. It runs at the rate it
//! asks the driver for ([`probe_at`]; 96 kHz unless asked, dasio#107): the
//! impulses are a second apart at that rate.

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use dasio_driver::ident::clsid;

use crate::vm_stall;

/// What a run measured.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub switches: u64,
    /// The samples the host's timeline ran (the driver's sample position
    /// at the end, from the first switch's).
    pub samples: u64,
    /// The blocks the driver counted the host missing in this run.
    pub missed: u64,
    pub emitted: u64,
    /// Impulse seconds the host's timeline jumped past (blocks the host
    /// missed or the device's pump skipped): no impulse is sent for them.
    pub skipped: u64,
    pub round_trips: Vec<u64>,
    pub late: u64,
    pub block: u32,
    /// The rate the host ran at (`getSampleRate` after its `setSampleRate`).
    pub rate: u32,
    pub latency_in: i32,
    pub latency_out: i32,
    /// The longest gap between two switches and the longest switch, µs.
    pub max_gap_us: u64,
    pub max_switch_us: u64,
    /// Switches more than 1 ms after the one before.
    pub gaps_over_1ms: u64,
    /// The blocks the host's long gaps span: for each gap over 1.5 blocks,
    /// its length in blocks, rounded up. A late block the device counts
    /// is the host's own when this covers it.
    pub gap_blocks: u64,
    /// RX 1's largest magnitude in each second, and the sample of it within
    /// the second (the impulses' arrivals, whatever their level).
    pub rx_peaks: Vec<(i32, u64)>,
    /// Impulses that never came back while the host itself lost its CPU
    /// on their way (a switch over 1 ms late between their emit and the
    /// next): a shared runner's VM, not the device (dasio#114). Any other
    /// lost impulse is the device's.
    pub lost_in_host_gap: u64,
    /// Diagnostics, not a rule (dasio#114): the watchdog's stalls (wakes
    /// over 1 ms late on its own timer, unpinned) and the longest, µs; the
    /// `gap_blocks` of host gaps a stall overlaps; the blocks the stalls
    /// span. A whole guest stopping shows here and in the gaps; a stall of
    /// the pump's CPU alone (CI 43779634868) only in the gaps.
    pub vm_stalls: u64,
    pub vm_stall_max_us: u64,
    pub vm_gap_blocks: u64,
    pub vm_stall_blocks: u64,
    /// The device pump's lost time in this run, by its own cause
    /// (dasio#114), in the host's whole blocks: lost to late wakes the
    /// device measured past its margin (the environment: the runner took
    /// the pump's CPU) and to anything else (the device). Only the first
    /// may excuse missing switches.
    pub pump_lost_late_blocks: u64,
    pub pump_lost_device_blocks: u64,
    /// The injected stall (dasio#162), all 0 without one: the device
    /// blocks published while the host stalled (its seq after less
    /// before).
    pub stall_published: u64,
    /// The stall's asked and measured length, µs.
    pub stall_asked_us: u64,
    pub stall_us: u64,
    /// The blocks the driver counted missed for the stall (at the first
    /// switch after it).
    pub stall_missed: u64,
    /// The pump's lost time by cause from the stall's start to
    /// [`STALL_LOOKS`] device ticks after it, in the host's whole blocks.
    pub stall_lost_late_blocks: u64,
    pub stall_lost_device_blocks: u64,
}

impl Report {
    /// The injected stall's figures, for [`crate::host_stall::verdict`].
    pub fn stall(&self) -> crate::host_stall::Stall {
        crate::host_stall::Stall {
            published: self.stall_published,
            asked_us: self.stall_asked_us,
            stall_us: self.stall_us,
            block: self.block,
            rate: self.rate,
            missed: self.stall_missed,
            lost_late_blocks: self.stall_lost_late_blocks,
            lost_device_blocks: self.stall_lost_device_blocks,
        }
    }
}

/// The onset of an arrival: a quarter of the impulse.
const ONSET: i32 = 1 << 28;
const IMPULSE: i32 = 1 << 30;

struct State {
    /// Samples a second at the host's rate: the impulses' spacing.
    rate: u64,
    bufs: Vec<[usize; 2]>,
    rx: usize,
    block: usize,
    /// The driver's sample position at the first switch.
    origin: Option<u64>,
    /// The samples from `origin` to this block (the DAW's timeline).
    position: u64,
    /// One host stall of this many ms, a few blocks after the first
    /// impulse left (while it is on its way): 0 for none.
    stall_ms: u64,
    stalled: bool,
    /// The device's figures around the stall; `None` without a stall.
    watch: Option<StallWatch>,
    next_emit: u64,
    last_emit: Option<u64>,
    /// When `last_emit` left, µs since the probe's start.
    last_emit_us: u64,
    /// Each lost impulse's way: its emit to the next emit, µs.
    lost: Vec<(u64, u64)>,
    round_trips: Vec<u64>,
    emitted: u64,
    skipped: u64,
    peaks: Vec<(i32, u64)>,
}

/// The device ticks (100 ms each) after the stall's start its lost time
/// is read at: a look publishes the blocks it judged at its own tick, or
/// at the next one when it saw a re-bootstrap before its blocks
/// (`pump_loss`), and the look in progress at the stall is the first.
pub const STALL_LOOKS: u32 = 3;

/// The device's header figures at a moment: its published seq, the
/// driver's missed count, the pump's lost time (late, device), µs.
#[derive(Clone, Copy)]
struct Figures {
    seq: u64,
    missed: u64,
    late_us: u64,
    device_us: u64,
}

/// The device's figures around the injected stall (dasio#162), read in
/// the switches (atomics only).
struct StallWatch {
    device: shm::Shared,
    /// At the stall's start; `None` before it.
    at: Option<Figures>,
    published: u64,
    stall_us: u64,
    /// The driver's missed count for the stall, once a switch after it
    /// came.
    missed: Option<u64>,
    /// The device's heartbeat last seen and its changes since the stall.
    beat: u64,
    beats: u32,
    /// The pump's lost time (late, device) since the stall, µs, once
    /// [`STALL_LOOKS`] ticks passed.
    lost: Option<(u64, u64)>,
}

impl StallWatch {
    fn figures(&self) -> Figures {
        let h = self.device.header();
        let (late_us, device_us) = pump_lost(h);
        Figures {
            seq: h.seq.load(Ordering::Acquire),
            missed: h.missed.load(Ordering::Relaxed),
            late_us,
            device_us,
        }
    }

    /// Each switch: the first after the stall takes the driver's missed
    /// count (it adds the blocks the host missed before it calls the
    /// switch); later ones count the device's ticks until the lost time
    /// is read.
    fn observe(&mut self) {
        let Some(at) = self.at else { return };
        let now = self.figures();
        if self.missed.is_none() {
            self.missed = Some(now.missed.saturating_sub(at.missed));
        } else if self.lost.is_none() {
            let beat = self.device.header().heartbeat_ms.load(Ordering::Acquire);
            if beat != self.beat {
                self.beat = beat;
                self.beats += 1;
                if self.beats >= STALL_LOOKS {
                    self.lost = Some(lost_since(at, now));
                }
            }
        }
    }

    /// The stall: `stall_ms` spun, its figures on either side.
    fn stall(&mut self, stall_ms: u64) {
        let at = self.figures();
        self.beat = self.device.header().heartbeat_ms.load(Ordering::Acquire);
        // Spun, not slept: a sleep rounds to the timer's tick (15.6 ms by
        // default), past the impulse's return.
        let from = Instant::now();
        let until = from + Duration::from_millis(stall_ms);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
        self.stall_us = from.elapsed().as_micros() as u64;
        self.published = self.figures().seq.saturating_sub(at.seq);
        self.at = Some(at);
    }
}

/// The pump's lost time (late, device) from `at` to `now`, µs.
fn lost_since(at: Figures, now: Figures) -> (u64, u64) {
    (
        now.late_us.saturating_sub(at.late_us),
        now.device_us.saturating_sub(at.device_us),
    )
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static MAX_GAP_US: AtomicU64 = AtomicU64::new(0);
static MAX_SWITCH_US: AtomicU64 = AtomicU64::new(0);
static GAPS_OVER_1MS: AtomicU64 = AtomicU64::new(0);
static GAP_BLOCKS: AtomicU64 = AtomicU64::new(0);
/// This run's block, µs (set before the driver starts).
static BLOCK_US: AtomicU64 = AtomicU64::new(0);
static LAST_SWITCH: Mutex<Option<Instant>> = Mutex::new(None);
static RUNNING: AtomicU32 = AtomicU32::new(0);
/// The probe's start: every span is µs since it.
static T0: Mutex<Option<Instant>> = Mutex::new(None);
/// The host's gaps over 1.5 blocks: `((start, end), blocks)`.
static GAPS: Mutex<Vec<((u64, u64), u64)>> = Mutex::new(Vec::new());
/// When each host gap over 1 ms ended (its switch came), µs.
static OVER_1MS_ENDS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// The watchdog runs while this is 1.
static WATCHING: AtomicU32 = AtomicU32::new(0);

/// µs from the probe's start to `t` (0 before the start).
fn since_start(t: Instant) -> u64 {
    T0.lock()
        .unwrap()
        .map_or(0, |t0| t.saturating_duration_since(t0).as_micros() as u64)
}

/// The DAW's callback with the driver's time info, as Ableton takes it:
/// the block's sample position is the host's timeline.
unsafe extern "C" fn switch_time(
    time: *mut ASIOTime,
    half: i32,
    direct: ASIOBool,
) -> *mut ASIOTime {
    // SAFETY: the driver's ASIOTime for this callback.
    let pos = unsafe { (*time).timeInfo.samplePosition }.get();
    // SAFETY: the same callback, timed.
    unsafe { timed(half, direct, Some(pos)) };
    core::ptr::null_mut()
}

unsafe extern "C" fn switch(half: i32, direct: ASIOBool) {
    // SAFETY: the same callback, timed.
    unsafe { timed(half, direct, None) };
}

unsafe fn timed(half: i32, direct: ASIOBool, pos: Option<u64>) {
    let entered = Instant::now();
    {
        let mut last = LAST_SWITCH.lock().unwrap();
        if let Some(t) = *last {
            let gap = entered.duration_since(t).as_micros() as u64;
            MAX_GAP_US.fetch_max(gap, Ordering::Relaxed);
            // Pre-sized at the start: no allocation in the switch until
            // their capacity runs out.
            if gap > 1000 {
                GAPS_OVER_1MS.fetch_add(1, Ordering::Relaxed);
                OVER_1MS_ENDS.lock().unwrap().push(since_start(entered));
            }
            let blocks = blocks_in_gap(gap, BLOCK_US.load(Ordering::Relaxed));
            GAP_BLOCKS.fetch_add(blocks, Ordering::Relaxed);
            if blocks > 0 {
                let span = (since_start(t), since_start(entered));
                GAPS.lock().unwrap().push((span, blocks));
            }
        }
        *last = Some(entered);
    }
    // SAFETY: the same callback, timed.
    unsafe { switch_body(half, direct, pos) };
    MAX_SWITCH_US.fetch_max(entered.elapsed().as_micros() as u64, Ordering::Relaxed);
}

/// The blocks a host gap of `gap_us` spans when it is over 1.5 blocks of
/// `block_us`, rounded up; 0 for a switch on time (one block, give or
/// take the scheduler's jitter).
pub fn blocks_in_gap(gap_us: u64, block_us: u64) -> u64 {
    if block_us == 0 || gap_us * 2 <= block_us * 3 {
        0
    } else {
        gap_us.div_ceil(block_us)
    }
}

unsafe fn switch_body(half: i32, _direct: ASIOBool, pos: Option<u64>) {
    let mut guard = STATE.lock().unwrap();
    let Some(s) = guard.as_mut() else { return };
    let h = half as usize;
    // The driver's timeline when it gives one: blocks the host missed are
    // in it, as a sound card's sample position counts them.
    if let Some(pos) = pos {
        let origin = *s.origin.get_or_insert(pos);
        s.position = pos.saturating_sub(origin);
    }
    if let Some(w) = s.watch.as_mut() {
        w.observe();
    }
    // An impulse second the timeline skipped is not sent.
    while s.next_emit < s.position {
        s.next_emit += s.rate;
        s.skipped += 1;
    }
    if s.stall_ms > 0
        && !s.stalled
        && s.last_emit
            .is_some_and(|e| s.position >= e + 4 * s.block as u64)
    {
        s.stalled = true;
        // The blocks the pump published while the host stalled, for the
        // driver to count missed, and the pump's lost time around them
        // (dasio#162).
        if let Some(w) = s.watch.as_mut() {
            w.stall(s.stall_ms);
        }
    }
    let n = s.block;
    // Inputs are bufs[0..rx], outputs bufs[rx..].
    let tx = s.bufs.len() - s.rx;
    for ch in 0..tx {
        let out = s.bufs[s.rx + ch][h] as *mut i32;
        if ch == 0 {
            for k in 0..n {
                let at = s.position + k as u64;
                let v = if at == s.next_emit { IMPULSE } else { 0 };
                // SAFETY: the driver's buffer of `n` samples.
                unsafe { out.add(k).write(v) };
            }
        } else if ch < s.rx {
            let inp = s.bufs[ch][h] as *const i32;
            // SAFETY: two buffers of `n` samples.
            unsafe { core::ptr::copy_nonoverlapping(inp, out, n) };
        }
    }
    if s.next_emit >= s.position && s.next_emit < s.position + n as u64 {
        // The impulse before never came back: its way, for the watchdog's
        // stalls to explain or not (at the end).
        let now = since_start(Instant::now());
        if s.last_emit.is_some() {
            s.lost.push((s.last_emit_us, now));
        }
        s.last_emit_us = now;
        s.last_emit = Some(s.next_emit);
        s.emitted += 1;
        s.next_emit += s.rate;
    }
    if s.rx > 0 {
        let inp = s.bufs[0][h] as *const i32;
        for k in 0..n {
            // SAFETY: the driver's buffer of `n` samples.
            let v = unsafe { inp.add(k).read() };
            let at = s.position + k as u64;
            let second = (at / s.rate) as usize;
            if s.peaks.len() <= second {
                s.peaks.resize(second + 1, (0, 0));
            }
            if v.unsigned_abs() > s.peaks[second].0.unsigned_abs() {
                s.peaks[second] = (v, at % s.rate);
            }
            if v.unsigned_abs() >= ONSET as u32
                && let Some(e) = s.last_emit.take()
            {
                s.round_trips.push(s.position + k as u64 - e);
            }
        }
    }
    s.position += n as u64;
    SWITCHES.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn message(sel: i32, val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    match (sel, val) {
        (K_ASIO_SELECTOR_SUPPORTED, K_ASIO_RESET_REQUEST | K_ASIO_SUPPORTS_TIME_INFO) => 1,
        (K_ASIO_SUPPORTS_TIME_INFO, _) => 1,
        (K_ASIO_RESET_REQUEST, _) => {
            RUNNING.store(0, Ordering::Release);
            1
        }
        _ => 0,
    }
}

/// Runs the probe on instance `instance` for `seconds` at 96 kHz.
pub fn probe(instance: u32, seconds: u64) -> Result<Report, String> {
    probe_at(instance, seconds, 0, 96_000)
}

/// [`probe`] with the host stalling once for `stall_ms` while the first
/// impulse is on its way (a DAW that lost its CPU): the round trip on the
/// host's timeline must not change.
pub fn probe_stalled(instance: u32, seconds: u64, stall_ms: u64) -> Result<Report, String> {
    probe_at(instance, seconds, stall_ms, 96_000)
}

/// [`probe_stalled`] with the host at `rate` Hz, set as a DAW sets its
/// project's rate (`canSampleRate`, `setSampleRate`) before it asks the
/// buffer size and the latencies (dasio#107: the device converts).
pub fn probe_at(instance: u32, seconds: u64, stall_ms: u64, rate: u32) -> Result<Report, String> {
    let id = clsid(instance).ok_or("no such instance")?;
    let mut factory: *mut c_void = core::ptr::null_mut();
    // SAFETY: valid pointers, as COM passes them.
    let hr = unsafe { dasio_driver::DllGetClassObject(&id, &IID_ICLASSFACTORY, &mut factory) };
    if hr != S_OK {
        return Err(format!("DllGetClassObject {hr:#x}"));
    }
    // SAFETY: the factory just made.
    let fv = unsafe { &**(factory as *const *const IClassFactoryVtbl) };
    let mut d: *mut c_void = core::ptr::null_mut();
    // SAFETY: as a host: the CLSID as the interface id.
    let hr = unsafe { (fv.CreateInstance)(factory, core::ptr::null_mut(), &id, &mut d) };
    // SAFETY: the factory's last reference.
    unsafe { (fv.Release)(factory) };
    if hr != S_OK {
        return Err(format!("CreateInstance {hr:#x}"));
    }
    // SAFETY: a live driver.
    let v = unsafe { &**(d as *const *const IAsioVtbl) };
    let mut report = Report::default();
    let missed_before;
    // SAFETY: the driver's methods as a host calls them.
    unsafe {
        if (v.init)(d, core::ptr::null_mut()) != ASIO_TRUE {
            (v.Release)(d);
            return Err("init refused".into());
        }
        let asked = f64::from(rate);
        let e = (v.canSampleRate)(d, asked);
        let e = if e == ASE_OK {
            (v.setSampleRate)(d, asked)
        } else {
            e
        };
        let mut got = 0.0f64;
        (v.getSampleRate)(d, &mut got);
        if e != ASE_OK || got != asked {
            let mut text: [core::ffi::c_char; 128] = [0; 128];
            (v.getErrorMessage)(d, text.as_mut_ptr());
            let text = core::ffi::CStr::from_ptr(text.as_ptr())
                .to_string_lossy()
                .into_owned();
            (v.Release)(d);
            return Err(format!(
                "the driver did not take {rate} Hz (ASIOError {e}, getSampleRate {got}): {text}"
            ));
        }
        report.rate = rate;
        let (mut ins, mut outs) = (0, 0);
        (v.getChannels)(d, &mut ins, &mut outs);
        let (mut min, mut max, mut pref, mut gran) = (0, 0, 0, 0);
        (v.getBufferSize)(d, &mut min, &mut max, &mut pref, &mut gran);
        (v.getLatencies)(d, &mut report.latency_in, &mut report.latency_out);
        report.block = pref as u32;
        let mut infos: Vec<ASIOBufferInfo> = (0..ins)
            .map(|c| (1, c))
            .chain((0..outs).map(|c| (0, c)))
            .map(|(i, c)| ASIOBufferInfo {
                isInput: i,
                channelNum: c,
                buffers: [core::ptr::null_mut(); 2],
            })
            .collect();
        let cb = ASIOCallbacks {
            bufferSwitch: Some(switch),
            sampleRateDidChange: None,
            asioMessage: Some(message),
            bufferSwitchTimeInfo: Some(switch_time),
        };
        let e = (v.createBuffers)(d, infos.as_mut_ptr(), infos.len() as i32, pref, &cb);
        if e != ASE_OK {
            return Err(format!("createBuffers {e}"));
        }
        // This run's counters only: a process may probe several instances.
        SWITCHES.store(0, Ordering::Relaxed);
        MAX_GAP_US.store(0, Ordering::Relaxed);
        MAX_SWITCH_US.store(0, Ordering::Relaxed);
        GAPS_OVER_1MS.store(0, Ordering::Relaxed);
        GAP_BLOCKS.store(0, Ordering::Relaxed);
        BLOCK_US.store(pref as u64 * 1_000_000 / u64::from(rate), Ordering::Relaxed);
        *LAST_SWITCH.lock().unwrap() = None;
        *GAPS.lock().unwrap() = Vec::with_capacity(4096);
        *OVER_1MS_ENDS.lock().unwrap() = Vec::with_capacity(4096);
        *T0.lock().unwrap() = Some(Instant::now());
        let watch = if stall_ms > 0 {
            match shm::Shared::open(instance) {
                Ok(device) => Some(StallWatch {
                    device,
                    at: None,
                    published: 0,
                    stall_us: 0,
                    missed: None,
                    beat: 0,
                    beats: 0,
                    lost: None,
                }),
                Err(e) => {
                    (v.disposeBuffers)(d);
                    (v.Release)(d);
                    return Err(format!(
                        "the device's shared memory for the stall's blocks: {e}"
                    ));
                }
            }
        } else {
            None
        };
        *STATE.lock().unwrap() = Some(State {
            rate: u64::from(rate),
            bufs: infos
                .iter()
                .map(|i| [i.buffers[0] as usize, i.buffers[1] as usize])
                .collect(),
            rx: ins as usize,
            block: pref as usize,
            origin: None,
            position: 0,
            stall_ms,
            stalled: false,
            watch,
            next_emit: u64::from(rate),
            last_emit: None,
            last_emit_us: 0,
            lost: Vec::with_capacity(64),
            round_trips: Vec::new(),
            emitted: 0,
            skipped: 0,
            peaks: Vec::new(),
        });
        missed_before = shm::Shared::open(instance)
            .map(|sh| sh.header().missed.load(Ordering::Relaxed))
            .unwrap_or(0);
        RUNNING.store(1, Ordering::Release);
    }
    // Ends the driver as a host does on a failed start: its buffers and
    // its reference.
    let abandon = |why: String| {
        RUNNING.store(0, Ordering::Release);
        // SAFETY: the driver made and its buffers created above.
        unsafe {
            (v.disposeBuffers)(d);
            (v.Release)(d);
        }
        STATE.lock().unwrap().take();
        Err(why)
    };
    let watchdog = match watchdog() {
        Ok(w) => w,
        Err(e) => return abandon(e),
    };
    // SAFETY: the driver's start, as a host calls it.
    if unsafe { (v.start)(d) } != ASE_OK {
        WATCHING.store(0, Ordering::Release);
        let _ = watchdog.join();
        return abandon("start refused".into());
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    // The pump's lost time counts from the second device tick after the
    // start, to the end of the run (dasio#114): the first tick's look may
    // have run before the start, the second's ran after it, and a look
    // publishes the blocks it judged at its own tick, so every block
    // published from there on was lost after the start.
    let lost_before = pump_lost_after_ticks(instance, 2);
    while Instant::now() < end && RUNNING.load(Ordering::Acquire) == 1 {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Before the stop: nothing the pump lost after the run is credited.
    let lost_after = shm::Shared::open(instance).map(|sh| pump_lost(sh.header()));
    // SAFETY: as a host ends.
    unsafe {
        (v.stop)(d);
        (v.disposeBuffers)(d);
        (v.Release)(d);
    }
    WATCHING.store(0, Ordering::Release);
    let stalls = watchdog
        .join()
        .map_err(|_| "the watchdog thread panicked".to_string())??;
    let lost_before = lost_before?;
    let lost_after =
        lost_after.map_err(|e| format!("the device's shared memory at the run's end: {e}"))?;
    let block = report.block;
    let host_blocks = |us: u64| crate::pump_loss::blocks_of(us, rate, block);
    report.pump_lost_late_blocks = host_blocks(lost_after.0.saturating_sub(lost_before.0));
    report.pump_lost_device_blocks = host_blocks(lost_after.1.saturating_sub(lost_before.1));
    if let Ok(shared) = shm::Shared::open(instance) {
        report.late = shared.header().late.load(Ordering::Relaxed);
        report.missed = shared
            .header()
            .missed
            .load(Ordering::Relaxed)
            .saturating_sub(missed_before);
    }
    let s = STATE.lock().unwrap().take().unwrap();
    report.samples = samples_run(&s);
    report.switches = SWITCHES.load(Ordering::Relaxed);
    report.max_gap_us = MAX_GAP_US.load(Ordering::Relaxed);
    report.max_switch_us = MAX_SWITCH_US.load(Ordering::Relaxed);
    report.gaps_over_1ms = GAPS_OVER_1MS.load(Ordering::Relaxed);
    report.gap_blocks = GAP_BLOCKS.load(Ordering::Relaxed);
    report.emitted = s.emitted;
    report.stall_asked_us = stall_ms * 1_000;
    if let Some(w) = s.watch.as_ref()
        && let Some(at) = w.at
    {
        // A run that ended before a switch after the stall, or before
        // STALL_LOOKS ticks, reads the rest at its end.
        let end = w.figures();
        let (late_us, device_us) = w.lost.unwrap_or_else(|| lost_since(at, end));
        report.stall_published = w.published;
        report.stall_us = w.stall_us;
        report.stall_missed = w.missed.unwrap_or(end.missed.saturating_sub(at.missed));
        report.stall_lost_late_blocks = host_blocks(late_us);
        report.stall_lost_device_blocks = host_blocks(device_us);
    }
    report.skipped = s.skipped;
    report.round_trips = s.round_trips;
    report.rx_peaks = s.peaks;
    let gaps = std::mem::take(&mut *GAPS.lock().unwrap());
    let ends = std::mem::take(&mut *OVER_1MS_ENDS.lock().unwrap());
    report.lost_in_host_gap = vm_stall::ended_within(&s.lost, &ends);
    report.vm_stalls = stalls.len() as u64;
    report.vm_stall_max_us = stalls.iter().map(|&(a, b)| b - a).max().unwrap_or(0);
    report.vm_gap_blocks = vm_stall::explained_blocks(&gaps, &stalls);
    report.vm_stall_blocks = vm_stall::stall_blocks(&stalls, BLOCK_US.load(Ordering::Relaxed));
    Ok(report)
}

/// The pump's lost time (late, device), µs, from the header.
fn pump_lost(h: &shm::Header) -> (u64, u64) {
    (
        h.pump_lost_late_us.load(Ordering::Relaxed),
        h.pump_lost_device_us.load(Ordering::Relaxed),
    )
}

/// The pump's lost time once the device's loop has ticked `ticks` more
/// times (it writes the lost time before its heartbeat).
fn pump_lost_after_ticks(instance: u32, ticks: u32) -> Result<(u64, u64), String> {
    let shared = shm::Shared::open(instance)
        .map_err(|e| format!("the device's shared memory for the pump's lost time: {e}"))?;
    let h = shared.header();
    // The loop ticks every 100 ms.
    let until = Instant::now() + Duration::from_secs(2);
    for _ in 0..ticks {
        let beat = h.heartbeat_ms.load(Ordering::Acquire);
        while h.heartbeat_ms.load(Ordering::Acquire) == beat {
            if Instant::now() >= until {
                return Err(format!(
                    "the device's loop did not tick {ticks} times in 2 s"
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(pump_lost(h))
}

/// The watchdog thread: its stalls, or why it could not run.
type Watchdog = std::thread::JoinHandle<Result<Vec<(u64, u64)>, String>>;

/// The VM-stall watchdog (dasio#114): a thread at TIME_CRITICAL in this
/// High-priority process, unpinned, waking each 1 ms on a high-resolution
/// timer; a wake over 1 ms late is a stall of the whole guest. The probe's
/// switches are driven by the device, so only this clock tells a VM stop
/// (both late) from a device stall (only the switches). Runs while
/// `WATCHING` is 1; returns its stalls, or why it could not run (the
/// probe then fails: without it nothing could be told apart).
fn watchdog() -> Result<Watchdog, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread,
        SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_TIME_CRITICAL, TIMER_ALL_ACCESS,
        WaitForSingleObject,
    };
    WATCHING.store(1, Ordering::Release);
    std::thread::Builder::new()
        .name("probe-watchdog".into())
        .spawn(|| {
            // SAFETY: this thread's pseudo handle.
            if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) } == 0
            {
                return Err(format!(
                    "the watchdog's priority: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: plain call: no security attributes, no name.
            let timer = unsafe {
                CreateWaitableTimerExW(
                    core::ptr::null(),
                    core::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            if timer.is_null() {
                return Err(format!(
                    "the watchdog's timer: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // Pre-sized: the loop never allocates until it is full.
            let mut stalls = Vec::with_capacity(4096);
            // 1 ms, relative: negative, in 100 ns units.
            let period: i64 = -10_000;
            let mut result = Ok(());
            while WATCHING.load(Ordering::Acquire) == 1 {
                let due = since_start(Instant::now()) + 1_000;
                // SAFETY: our timer and a due time that outlives the call;
                // no completion routine, no resume.
                let armed =
                    unsafe { SetWaitableTimer(timer, &period, 0, None, core::ptr::null(), 0) } != 0;
                // SAFETY: our timer, armed above; bounded, never INFINITE.
                if !armed || unsafe { WaitForSingleObject(timer, 1_000) } == WAIT_FAILED {
                    result = Err(format!(
                        "the watchdog's wait: {}",
                        std::io::Error::last_os_error()
                    ));
                    break;
                }
                if let Some(stall) = vm_stall::stall_of(due, since_start(Instant::now())) {
                    stalls.push(stall);
                }
            }
            // SAFETY: our timer, closed once.
            unsafe { CloseHandle(timer) };
            result.map(|()| stalls)
        })
        .map_err(|e| format!("the watchdog thread: {e}"))
}

/// This process at High priority, as a DAW runs (Ableton does).
pub fn high_priority() -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass,
    };
    // SAFETY: the pseudo handle of this process.
    if unsafe { SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) } == 0 {
        return Err(format!(
            "High priority: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// This process's CPUs (`mask`, one bit a CPU of the first group).
pub fn set_affinity(mask: usize) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessAffinityMask};
    // SAFETY: the pseudo handle of this process and a mask.
    if unsafe { SetProcessAffinityMask(GetCurrentProcess(), mask) } == 0 {
        return Err(format!(
            "the affinity {mask:#x}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// The samples the host's timeline ran, the missed blocks in it (the
/// driver's timeline), at the run's end: `switch_body` moved `position`
/// past the last block it served, so that is the end (dasio#203).
fn samples_run(s: &State) -> u64 {
    s.position
}

#[cfg(test)]
mod samples_tests {
    use super::*;

    #[test]
    fn the_samples_the_host_ran_end_with_its_last_block() {
        // dasio#203 (CI 44494755515): 12000 switches of 32 ran 384000
        // samples (4 s at 96 kHz), the report said 384032, and the e2e
        // check wanted the 4 s impulse of a block that never ran.
        let block = 32;
        let mut halves = [[0i32; 32]; 2];
        let bufs = vec![[
            halves[0].as_mut_ptr() as usize,
            halves[1].as_mut_ptr() as usize,
        ]];
        *STATE.lock().unwrap() = Some(State {
            rate: 96_000,
            bufs,
            rx: 0,
            block,
            origin: None,
            position: 0,
            stall_ms: 0,
            stalled: false,
            watch: None,
            next_emit: 96_000,
            last_emit: None,
            last_emit_us: 0,
            lost: Vec::new(),
            round_trips: Vec::new(),
            emitted: 0,
            skipped: 0,
            peaks: Vec::new(),
        });
        for k in 0..3 {
            // SAFETY: the two halves above, `block` samples each, live
            // through the calls.
            unsafe { switch_body(k % 2, 0, None) };
        }
        let s = STATE.lock().unwrap().take().unwrap();
        assert_eq!(samples_run(&s), 3 * block as u64);
        assert_eq!(s.emitted, 0);
    }
}

#[cfg(test)]
mod gap_tests {
    use super::blocks_in_gap;

    #[test]
    fn a_switch_on_time_spans_no_lost_block() {
        // Block 32 at 96 kHz: 333 us; jitter up to 1.5 blocks is on time.
        assert_eq!(blocks_in_gap(333, 333), 0);
        assert_eq!(blocks_in_gap(499, 333), 0);
        // Exactly 1.5 blocks still counts as on time; past it, not.
        assert_eq!(blocks_in_gap(501, 334), 0);
        assert_eq!(blocks_in_gap(502, 334), 2);
    }

    #[test]
    fn a_long_gap_counts_its_blocks_rounded_up() {
        // 763 us at block 32 (CI 41340984953): three blocks.
        assert_eq!(blocks_in_gap(763, 333), 3);
        assert_eq!(blocks_in_gap(501, 333), 2);
        assert_eq!(blocks_in_gap(666, 333), 2);
        assert_eq!(blocks_in_gap(48_900, 666), 74);
    }

    #[test]
    fn no_block_counts_nothing() {
        assert_eq!(blocks_in_gap(5_000, 0), 0);
    }
}
