//! `dante-asio probe` (plan Task 10): an ASIO host on the instance's driver
//! that measures the round trip through Dante. TX 1 sends one impulse a
//! second (one sample at half scale, silence else); RX 1 (subscribed to our
//! own TX 1 for the test) is scanned for it; each arrival's distance from
//! its emit is a round trip in samples. Inputs 2.. are passed to outputs
//! 2.. unchanged. The driver is reached as a DAW reaches it (its COM entry,
//! the CLSID as the interface), in this process.

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dante_asio_driver::abi::*;
use dante_asio_driver::ident::clsid;

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
}

/// The onset of an arrival: a quarter of the impulse.
const ONSET: i32 = 1 << 28;
const IMPULSE: i32 = 1 << 30;

struct State {
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
    next_emit: u64,
    last_emit: Option<u64>,
    round_trips: Vec<u64>,
    emitted: u64,
    skipped: u64,
    peaks: Vec<(i32, u64)>,
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
            if gap > 1000 {
                GAPS_OVER_1MS.fetch_add(1, Ordering::Relaxed);
            }
            GAP_BLOCKS.fetch_add(
                blocks_in_gap(gap, BLOCK_US.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
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
    // An impulse second the timeline skipped is not sent.
    while s.next_emit < s.position {
        s.next_emit += 96_000;
        s.skipped += 1;
    }
    if s.stall_ms > 0
        && !s.stalled
        && s.last_emit
            .is_some_and(|e| s.position >= e + 4 * s.block as u64)
    {
        s.stalled = true;
        // Spun, not slept: a sleep rounds to the timer's tick (15.6 ms by
        // default), past the impulse's return.
        let until = Instant::now() + Duration::from_millis(s.stall_ms);
        while Instant::now() < until {
            std::hint::spin_loop();
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
        s.last_emit = Some(s.next_emit);
        s.emitted += 1;
        s.next_emit += 96_000;
    }
    if s.rx > 0 {
        let inp = s.bufs[0][h] as *const i32;
        for k in 0..n {
            // SAFETY: the driver's buffer of `n` samples.
            let v = unsafe { inp.add(k).read() };
            let at = s.position + k as u64;
            let second = (at / 96_000) as usize;
            if s.peaks.len() <= second {
                s.peaks.resize(second + 1, (0, 0));
            }
            if v.unsigned_abs() > s.peaks[second].0.unsigned_abs() {
                s.peaks[second] = (v, at % 96_000);
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

/// Runs the probe on instance `instance` for `seconds`.
pub fn probe(instance: u32, seconds: u64) -> Result<Report, String> {
    probe_stalled(instance, seconds, 0)
}

/// [`probe`] with the host stalling once for `stall_ms` while the first
/// impulse is on its way (a DAW that lost its CPU): the round trip on the
/// host's timeline must not change.
pub fn probe_stalled(instance: u32, seconds: u64, stall_ms: u64) -> Result<Report, String> {
    let id = clsid(instance).ok_or("no such instance")?;
    let mut factory: *mut c_void = core::ptr::null_mut();
    // SAFETY: valid pointers, as COM passes them.
    let hr = unsafe { dante_asio_driver::DllGetClassObject(&id, &IID_ICLASSFACTORY, &mut factory) };
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
            return Err("init refused".into());
        }
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
        BLOCK_US.store(pref as u64 * 1_000_000 / 96_000, Ordering::Relaxed);
        *LAST_SWITCH.lock().unwrap() = None;
        *STATE.lock().unwrap() = Some(State {
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
            next_emit: 96_000,
            last_emit: None,
            round_trips: Vec::new(),
            emitted: 0,
            skipped: 0,
            peaks: Vec::new(),
        });
        missed_before = shm::Shared::open(instance)
            .map(|sh| sh.header().missed.load(Ordering::Relaxed))
            .unwrap_or(0);
        RUNNING.store(1, Ordering::Release);
        if (v.start)(d) != ASE_OK {
            return Err("start refused".into());
        }
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end && RUNNING.load(Ordering::Acquire) == 1 {
        std::thread::sleep(Duration::from_millis(100));
    }
    // SAFETY: as a host ends.
    unsafe {
        (v.stop)(d);
        (v.disposeBuffers)(d);
        (v.Release)(d);
    }
    if let Ok(shared) = shm::Shared::open(instance) {
        report.late = shared.header().late.load(Ordering::Relaxed);
        report.missed = shared
            .header()
            .missed
            .load(Ordering::Relaxed)
            .saturating_sub(missed_before);
    }
    let s = STATE.lock().unwrap().take().unwrap();
    report.samples = s.position + s.block as u64;
    report.switches = SWITCHES.load(Ordering::Relaxed);
    report.max_gap_us = MAX_GAP_US.load(Ordering::Relaxed);
    report.max_switch_us = MAX_SWITCH_US.load(Ordering::Relaxed);
    report.gaps_over_1ms = GAPS_OVER_1MS.load(Ordering::Relaxed);
    report.gap_blocks = GAP_BLOCKS.load(Ordering::Relaxed);
    report.emitted = s.emitted;
    report.skipped = s.skipped;
    report.round_trips = s.round_trips;
    report.rx_peaks = s.peaks;
    Ok(report)
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
