//! One instance's ASIO driver object: the IASIO methods and the worker
//! thread that drives the DAW's callbacks.
//!
//! - With the instance's device process running, the buffers the DAW gets
//!   are the shared memory's two halves (no copy): per published block the
//!   worker calls the DAW on half `seq % 2`, then stores `ack` and signals
//!   ready (at once when the DAW calls `outputReady`, else when its callback
//!   returns).
//! - With no device (or a device that went quiet for 50 ms), the worker
//!   free-runs: local buffers of silence, one callback a block period, and a
//!   `kAsioResetRequest` to the DAW once a device is there, so it opens the
//!   real buffers.
//! - Nothing the driver does reaches the DAW as a crash or a hang: every
//!   method runs in [`guard`], locks survive a panic, and a worker stuck in
//!   the DAW's own callback is abandoned after [`STOP_WAIT`].
//! - Once a second the worker reads the device's state from the header
//!   ([`Watch`]): a device back with another shape gets the DAW a
//!   `kAsioResetRequest`, within the instance's [`Budget`]; blocks the
//!   worker skipped are counted in the header's `missed`.

use core::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use shm::{DriverMessage, HostUse, Shape, Shared};
use windows_sys::Win32::System::Threading::{
    AVRT_PRIORITY_CRITICAL, AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, GetCurrentThread,
    GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};

use crate::abi::*;
use crate::com::OBJECTS;
use crate::guard::{guard, join_within, lock};
use crate::ident::{MAX_INSTANCES, driver_name, instance_of};
use crate::liveness::{Facts, Verdict, Watch};
use crate::reset::Budget;
use crate::systime;

/// The rate: Dante at 96 kHz.
pub const RATE: f64 = 96_000.0;
/// The shape a DAW sees while no device runs (it asks again after a reset).
pub const NO_DEVICE_SHAPE: Shape = Shape {
    rx: 8,
    tx: 8,
    block: 32,
};
/// Spin rounds before the worker sleeps on the block event (a few µs).
const SPIN_ROUNDS: u32 = 2_000;
/// A device silent this long is treated as gone (free-run).
const QUIET: Duration = Duration::from_millis(50);
/// The longest a DAW's thread waits for the worker to end; past it the
/// worker (stuck in the DAW's callback) is abandoned.
pub const STOP_WAIT: Duration = Duration::from_secs(1);
/// How often the worker reads the device's state.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Each instance's reset requests in this process (a host makes a new
/// driver object after a reset, so the budget outlives the object).
static BUDGETS: Mutex<[Budget; MAX_INSTANCES as usize]> =
    Mutex::new([const { Budget::new() }; MAX_INSTANCES as usize]);

#[repr(C)]
pub struct Driver {
    vtbl: *const IAsioVtbl,
    refs: AtomicU32,
    instance: u32,
    state: Mutex<State>,
    rt: Arc<Rt>,
}

/// What the worker and `outputReady` share.
struct Rt {
    /// The DAW called `outputReady` for the block in hand.
    ready_sent: AtomicBool,
    /// The DAW said it calls `outputReady` (it may skip it under load).
    seq_in_hand: AtomicU64,
    /// When the driver called the DAW's callback for the block in hand
    /// (`shm::qpc_ns`, just before the call: the few µs of our own preamble
    /// count as processing): it goes with that block's ack (dasio#22).
    entered_ns: AtomicU64,
    /// The worker's thread id: `outputReady` on that thread (inside the
    /// callback, as ASIO hosts call it) pairs the start; on any other
    /// thread the worker may already be on the next block, so the start
    /// goes as unknown rather than wrong.
    worker_tid: AtomicU32,
    /// Samples handed to the DAW since `start`.
    position: AtomicU64,
    /// The running worker's generation (0: none), so the position
    /// advances: set by `start`, cleared by `stop_worker` and by a worker
    /// that ends on its own (only its own generation, never a newer
    /// worker's). `getSamplePosition` reads it lock-free: hosts call it
    /// inside their callback, while a host's `stop` may hold the state lock
    /// waiting for that callback.
    running: AtomicU64,
    /// Workers started: the next one's generation.
    starts: AtomicU64,
    /// The last hand-over's `systemTime`, ns in timeGetTime's base
    /// ([`systime`]).
    systime_ns: AtomicU64,
    /// The shift from counter ns into timeGetTime's base, measured at
    /// `start` ([`systime::calibrate`]).
    systime_offset: AtomicU64,
    /// The instance's mapping: opened by `init`, and opened again by an
    /// `init` after the device published a new session (a host that
    /// re-inits the same object after a reset, instead of making a new one).
    shm: Mutex<Option<Arc<Shared>>>,
    /// The device's last verdict (`Verdict as u8 + 1`; 0 before any).
    verdict: AtomicU8,
    /// Device restarts the worker saw.
    restarts: AtomicU32,
}

impl Rt {
    /// The mapping now (a clone of its Arc: never held across a call).
    fn shm(&self) -> Option<Arc<Shared>> {
        lock(&self.shm).clone()
    }
}

#[derive(Default)]
struct State {
    error: String,
    shape: Option<Shape>,
    created: Option<Created>,
    worker: Option<Worker>,
}

/// The running worker and its own stop flag (an abandoned worker keeps its
/// flag set; the next one gets a new one).
struct Worker {
    handle: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

struct Created {
    callbacks: ASIOCallbacks,
    time_info: bool,
    /// The channels whose buffers `createBuffers` made, as (input, channel):
    /// the only ones `getChannelInfo` reports active.
    active: Vec<(bool, u32)>,
    /// Free-run buffers (no device): [half][channel] of `block` samples.
    local: Option<Vec<Vec<Box<[i32]>>>>,
}

// SAFETY: the callbacks are the DAW's plain function pointers, callable from
// any thread (the SDK's contract); the local buffers are owned data.
unsafe impl Send for Created {}

static VTBL: IAsioVtbl = IAsioVtbl {
    QueryInterface: query_interface,
    AddRef: add_ref,
    Release: release,
    init,
    getDriverName: get_driver_name,
    getDriverVersion: get_driver_version,
    getErrorMessage: get_error_message,
    start,
    stop,
    getChannels: get_channels,
    getLatencies: get_latencies,
    getBufferSize: get_buffer_size,
    canSampleRate: can_sample_rate,
    getSampleRate: get_sample_rate,
    setSampleRate: set_sample_rate,
    getClockSources: get_clock_sources,
    setClockSource: set_clock_source,
    getSamplePosition: get_sample_position,
    getChannelInfo: get_channel_info,
    createBuffers: create_buffers,
    disposeBuffers: dispose_buffers,
    controlPanel: control_panel,
    future,
    outputReady: output_ready,
};

impl Driver {
    /// A new driver for `instance` with one reference, as COM hands it out.
    pub(crate) fn new_raw(instance: u32) -> *mut c_void {
        OBJECTS.fetch_add(1, Ordering::AcqRel);
        Box::into_raw(Box::new(Driver {
            vtbl: &VTBL,
            refs: AtomicU32::new(1),
            instance,
            state: Mutex::new(State::default()),
            rt: Arc::new(Rt {
                ready_sent: AtomicBool::new(false),
                seq_in_hand: AtomicU64::new(0),
                entered_ns: AtomicU64::new(0),
                worker_tid: AtomicU32::new(0),
                position: AtomicU64::new(0),
                running: AtomicU64::new(0),
                starts: AtomicU64::new(0),
                systime_ns: AtomicU64::new(0),
                systime_offset: AtomicU64::new(0),
                shm: Mutex::new(None),
                verdict: AtomicU8::new(0),
                restarts: AtomicU32::new(0),
            }),
        }))
        .cast()
    }

    fn shape(&self) -> Shape {
        lock(&self.state).shape.unwrap_or(NO_DEVICE_SHAPE)
    }

    fn latencies(&self) -> (i32, i32) {
        match self.rt.shm() {
            Some(s) => {
                let h = s.header();
                (
                    h.latency_in.load(Ordering::Relaxed) as i32,
                    h.latency_out.load(Ordering::Relaxed) as i32,
                )
            }
            None => {
                let b = NO_DEVICE_SHAPE.block as i32;
                (b, b)
            }
        }
    }

    /// Stops the worker, waiting at most [`STOP_WAIT`]. A worker still in
    /// the DAW's callback after that is abandoned: its flag stays set, and
    /// the free-run buffers the DAW may still be writing are leaked, never
    /// freed under it.
    fn stop_worker(&self, state: &mut State) {
        let Some(w) = state.worker.take() else {
            return;
        };
        self.rt.running.store(0, Ordering::Release);
        // No more answers: the device stops waiting for them first.
        if let Some(s) = self.rt.shm() {
            s.header().host_attached.store(0, Ordering::Release);
            s.set_host_pid(0);
        }
        w.stop.store(true, Ordering::Release);
        // The host stops us from inside its own callback, on the worker's
        // thread (dasio#28 E16): the worker ends as soon as that callback
        // returns, without another call into the host. It cannot be joined
        // from inside itself; it returns through this DLL's code, so it
        // counts as a live object for good, as a hung one does.
        // SAFETY: plain call.
        if self.rt.worker_tid.load(Ordering::Acquire) == unsafe { GetCurrentThreadId() } {
            drop(w.handle);
            crate::com::OBJECTS.fetch_add(1, Ordering::AcqRel);
            return;
        }
        // A handle handed back is dropped here: the thread is left running.
        if join_within(w.handle, STOP_WAIT).is_err() {
            if let Some(local) = state.created.as_mut().and_then(|c| c.local.take()) {
                std::mem::forget(local);
            }
            // The thread returns into this DLL's code later: it counts as a
            // live object for good, so DllCanUnloadNow never lets the host
            // unmap the DLL under it.
            crate::com::OBJECTS.fetch_add(1, Ordering::AcqRel);
            state.error = format!(
                "the host's callback did not return within {} ms: its thread was left",
                STOP_WAIT.as_millis()
            );
        }
    }
}

/// # Safety
/// `this` is a live Driver.
unsafe fn me<'a>(this: *mut c_void) -> &'a Driver {
    // SAFETY: the caller's.
    unsafe { &*(this as *const Driver) }
}

fn write_c(dst: *mut c_char, text: &str, cap: usize) {
    if dst.is_null() {
        return;
    }
    let bytes = text.as_bytes();
    let n = bytes.len().min(cap - 1);
    // SAFETY: the SDK gives buffers of `cap` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), dst, n);
        *dst.add(n) = 0;
    }
}

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    guard(E_UNEXPECTED, || {
        if ppv.is_null() || riid.is_null() {
            return E_POINTER;
        }
        // SAFETY: COM's valid pointers; `this` a live Driver.
        let (iid, d) = unsafe { (*riid, me(this)) };
        if iid == IID_IUNKNOWN || instance_of(&iid) == Some(d.instance) {
            d.refs.fetch_add(1, Ordering::AcqRel);
            // SAFETY: checked non-null.
            unsafe { *ppv = this };
            S_OK
        } else {
            // SAFETY: checked non-null.
            unsafe { *ppv = core::ptr::null_mut() };
            E_NOINTERFACE
        }
    })
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    // SAFETY: a live Driver.
    unsafe { me(this) }.refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    guard(0, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let left = d.refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            {
                let mut st = lock(&d.state);
                d.stop_worker(&mut st);
                if let Some(s) = d.rt.shm() {
                    s.header().host_attached.store(0, Ordering::Release);
                    s.set_host_pid(0);
                }
            }
            // SAFETY: the last reference: the Box this driver came from.
            drop(unsafe { Box::from_raw(this as *mut Driver) });
            OBJECTS.fetch_sub(1, Ordering::AcqRel);
        }
        left
    })
}

unsafe extern "system" fn init(this: *mut c_void, _sys: *mut c_void) -> ASIOBool {
    guard(ASIO_FALSE, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let mut st = lock(&d.state);
        if let Some(s) = d.rt.shm() {
            // The same object re-inited: keep the mapping unless the device
            // moved to a new session (and no buffers of the old one are out).
            if s.published_session() == s.session() || st.created.is_some() {
                st.shape = Some(s.shape());
                return ASIO_TRUE;
            }
        }
        match Shared::open(d.instance) {
            Ok(s) => {
                st.shape = Some(s.shape());
                *lock(&d.rt.shm) = Some(Arc::new(s));
            }
            Err(e) => {
                st.shape = None;
                // No stale mapping under a free-running driver.
                *lock(&d.rt.shm) = None;
                st.error = format!("no dasio device {} running ({e})", d.instance);
            }
        }
        // A DAW must be able to open the driver without its device: it free-runs.
        ASIO_TRUE
    })
}

unsafe extern "system" fn get_driver_name(this: *mut c_void, name: *mut c_char) {
    guard((), || {
        // SAFETY: a live Driver.
        write_c(name, &driver_name(unsafe { me(this) }.instance), 32);
    })
}

unsafe extern "system" fn get_driver_version(_this: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn get_error_message(this: *mut c_void, text: *mut c_char) {
    guard((), || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let e = lock(&d.state).error.clone();
        let device = device_text(
            d.rt.verdict.load(Ordering::Acquire),
            d.rt.restarts.load(Ordering::Acquire),
        );
        let text_all = match (e.is_empty(), device.is_empty()) {
            (_, true) => e,
            (true, false) => device,
            (false, false) => format!("{e}; {device}"),
        };
        write_c(text, &text_all, 124);
    })
}

unsafe extern "system" fn start(this: *mut c_void) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let mut st = lock(&d.state);
        if st.worker.is_some() {
            return ASE_OK;
        }
        let Some(created) = st.created.as_ref() else {
            return ASE_INVALID_MODE;
        };
        let time_info = created.time_info;
        let stop = Arc::new(AtomicBool::new(false));
        let generation = d.rt.starts.fetch_add(1, Ordering::AcqRel) + 1;
        let job = Job {
            callbacks: created.callbacks,
            time_info,
            shape: st.shape.unwrap_or(NO_DEVICE_SHAPE),
            instance: d.instance,
            rt: Arc::clone(&d.rt),
            stop: Arc::clone(&stop),
            generation,
            seen: d
                .rt
                .shm()
                .map_or(0, |s| s.header().seq.load(Ordering::Acquire)),
        };
        d.rt.position.store(0, Ordering::Relaxed);
        // The new worker stores its own id; until then no stale one matches.
        d.rt.worker_tid.store(0, Ordering::Release);
        d.rt.systime_offset
            .store(systime::calibrate(), Ordering::Release);
        // Before the spawn: the worker's first callback may ask the position.
        d.rt.running.store(generation, Ordering::Release);
        match std::thread::Builder::new()
            .name("dasio".into())
            .spawn(move || job.run())
        {
            Ok(handle) => {
                st.worker = Some(Worker { handle, stop });
                // From now on the host answers blocks: the device waits for
                // each answer (and counts a missing one as late).
                if let Some(s) = d.rt.shm() {
                    if time_info {
                        s.header().note_use(HostUse::TimeInfo);
                    }
                    s.header().host_attached.store(1, Ordering::Release);
                    s.set_host_pid(std::process::id());
                }
                ASE_OK
            }
            Err(e) => {
                d.rt.running.store(0, Ordering::Release);
                st.error = format!("the worker thread did not start ({e})");
                ASE_HW_MALFUNCTION
            }
        }
    })
}

unsafe extern "system" fn stop(this: *mut c_void) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let mut st = lock(&d.state);
        d.stop_worker(&mut st);
        ASE_OK
    })
}

unsafe extern "system" fn get_channels(
    this: *mut c_void,
    ins: *mut i32,
    outs: *mut i32,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let s = unsafe { me(this) }.shape();
        if ins.is_null() || outs.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // SAFETY: checked non-null.
        unsafe {
            *ins = s.rx as i32;
            *outs = s.tx as i32;
        }
        ASE_OK
    })
}

unsafe extern "system" fn get_latencies(
    this: *mut c_void,
    ins: *mut i32,
    outs: *mut i32,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let (i, o) = d.latencies();
        if ins.is_null() || outs.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // The device logs whether the DAW reads them again after a reset
        // or a latency change (dasio#28 E4).
        if let Some(s) = d.rt.shm() {
            s.header().note_latency_read();
        }
        // SAFETY: checked non-null.
        unsafe {
            *ins = i;
            *outs = o;
        }
        ASE_OK
    })
}

unsafe extern "system" fn get_buffer_size(
    this: *mut c_void,
    min: *mut i32,
    max: *mut i32,
    preferred: *mut i32,
    granularity: *mut i32,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let b = unsafe { me(this) }.shape().block as i32;
        if min.is_null() || max.is_null() || preferred.is_null() || granularity.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // The instance's block only: its tray sets it (the device's pump runs it).
        // SAFETY: checked non-null.
        unsafe {
            *min = b;
            *max = b;
            *preferred = b;
            *granularity = 0;
        }
        ASE_OK
    })
}

unsafe extern "system" fn can_sample_rate(_this: *mut c_void, rate: f64) -> ASIOError {
    if rate == RATE { ASE_OK } else { ASE_NO_CLOCK }
}

unsafe extern "system" fn get_sample_rate(_this: *mut c_void, rate: *mut f64) -> ASIOError {
    if rate.is_null() {
        return ASE_INVALID_PARAMETER;
    }
    // SAFETY: checked non-null.
    unsafe { *rate = RATE };
    ASE_OK
}

unsafe extern "system" fn set_sample_rate(_this: *mut c_void, rate: f64) -> ASIOError {
    if rate == RATE || rate == 0.0 {
        ASE_OK
    } else {
        ASE_NO_CLOCK
    }
}

unsafe extern "system" fn get_clock_sources(
    _this: *mut c_void,
    clocks: *mut ASIOClockSource,
    n: *mut i32,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        if clocks.is_null() || n.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // SAFETY: the host gives at least one entry and its count.
        unsafe {
            let c = &mut *clocks;
            c.index = 0;
            c.associatedChannel = -1;
            c.associatedGroup = -1;
            c.isCurrentSource = ASIO_TRUE;
            write_c(c.name.as_mut_ptr(), "Dante (PTP)", 32);
            *n = 1;
        }
        ASE_OK
    })
}

unsafe extern "system" fn set_clock_source(_this: *mut c_void, index: i32) -> ASIOError {
    if index == 0 {
        ASE_OK
    } else {
        ASE_INVALID_PARAMETER
    }
}

unsafe extern "system" fn get_sample_position(
    this: *mut c_void,
    pos: *mut ASIOSamples,
    stamp: *mut ASIOTimeStamp,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        if pos.is_null() || stamp.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // SAFETY: a live Driver; checked non-null.
        let d = unsafe { me(this) };
        unsafe {
            *pos = Asio64::new(d.rt.position.load(Ordering::Acquire));
            *stamp = Asio64::new(d.rt.systime_ns.load(Ordering::Acquire));
        }
        // The SDK: with no worker the position does not advance (before
        // `start`, after `stop`, after a worker that ended on its own).
        if d.rt.running.load(Ordering::Acquire) != 0 {
            ASE_OK
        } else {
            ASE_SP_NOT_ADVANCING
        }
    })
}

unsafe extern "system" fn get_channel_info(
    this: *mut c_void,
    info: *mut ASIOChannelInfo,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        if info.is_null() {
            return ASE_INVALID_PARAMETER;
        }
        // SAFETY: a live Driver; the host's struct with `channel` and `isInput` set.
        let (d, i) = unsafe { (me(this), &mut *info) };
        let (ch, input) = (i.channel, i.isInput != 0);
        let st = lock(&d.state);
        let s = st.shape.unwrap_or(NO_DEVICE_SHAPE);
        let count = if input { s.rx } else { s.tx } as i32;
        if ch < 0 || ch >= count {
            return ASE_INVALID_PARAMETER;
        }
        // The SDK: active only while createBuffers' buffers exist for it.
        let active = st
            .created
            .as_ref()
            .is_some_and(|c| c.active.contains(&(input, ch as u32)));
        drop(st);
        i.isActive = if active { ASIO_TRUE } else { ASIO_FALSE };
        i.channelGroup = 0;
        i.type_ = ASIOST_INT32_LSB;
        let name = format!("{} {}", if input { "RX" } else { "TX" }, ch + 1);
        write_c(i.name.as_mut_ptr(), &name, 32);
        ASE_OK
    })
}

unsafe extern "system" fn create_buffers(
    this: *mut c_void,
    infos: *mut ASIOBufferInfo,
    n: i32,
    size: i32,
    callbacks: *const ASIOCallbacks,
) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        if infos.is_null() || callbacks.is_null() || n <= 0 {
            return ASE_INVALID_PARAMETER;
        }
        // SAFETY: a live Driver; the host's `n` buffer infos and its callbacks.
        let (d, infos, callbacks) = unsafe {
            (
                me(this),
                core::slice::from_raw_parts_mut(infos, n as usize),
                *callbacks,
            )
        };
        let mut st = lock(&d.state);
        d.stop_worker(&mut st);
        let shape = st.shape.unwrap_or(NO_DEVICE_SHAPE);
        if size != shape.block as i32 {
            return ASE_INVALID_MODE;
        }
        let shm = d.rt.shm();
        let mut local: Vec<Vec<Box<[i32]>>> = vec![Vec::new(), Vec::new()];
        let mut active = Vec::with_capacity(infos.len());
        for info in infos.iter_mut() {
            let input = info.isInput != 0;
            let ch = info.channelNum;
            let count = if input { shape.rx } else { shape.tx } as i32;
            if ch < 0 || ch >= count {
                return ASE_INVALID_PARAMETER;
            }
            let ch = ch as u32;
            let mut bufs = [core::ptr::null_mut(); 2];
            for (half, b) in bufs.iter_mut().enumerate() {
                *b = match shm.as_ref() {
                    Some(s) => {
                        let Some(p) = s.buffer(half as u32, input, ch) else {
                            return ASE_INVALID_PARAMETER;
                        };
                        if !input {
                            // A reconnecting DAW never hears the last
                            // session's block.
                            // SAFETY: a buffer of `block` samples in the
                            // mapping; no worker runs (stopped above).
                            unsafe { core::ptr::write_bytes(p, 0, shape.block as usize) };
                        }
                        p.cast()
                    }
                    None => {
                        let mut v = vec![0i32; shape.block as usize].into_boxed_slice();
                        let p = v.as_mut_ptr().cast();
                        local[half].push(v);
                        p
                    }
                };
            }
            info.buffers = bufs;
            active.push((input, ch));
        }
        // SAFETY: the host's callback, asked as the SDK says.
        let time_info = callbacks.asioMessage.is_some_and(|m| unsafe {
            m(
                K_ASIO_SELECTOR_SUPPORTED,
                K_ASIO_SUPPORTS_TIME_INFO,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) == 1
                && m(
                    K_ASIO_SUPPORTS_TIME_INFO,
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ) == 1
        });
        st.created = Some(Created {
            callbacks,
            time_info,
            active,
            local: shm.is_none().then_some(local),
        });
        ASE_OK
    })
}

unsafe extern "system" fn dispose_buffers(this: *mut c_void) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let d = unsafe { me(this) };
        let mut st = lock(&d.state);
        d.stop_worker(&mut st);
        st.created = None;
        if let Some(s) = d.rt.shm() {
            s.header().host_attached.store(0, Ordering::Release);
            s.set_host_pid(0);
        }
        ASE_OK
    })
}

unsafe extern "system" fn control_panel(_this: *mut c_void) -> ASIOError {
    // The settings live in the instance's tray icon.
    ASE_OK
}

unsafe extern "system" fn future(
    _this: *mut c_void,
    selector: i32,
    _opt: *mut c_void,
) -> ASIOError {
    match selector {
        K_ASIO_CAN_TIME_INFO => ASE_SUCCESS,
        _ => ASE_NOT_PRESENT,
    }
}

unsafe extern "system" fn output_ready(this: *mut c_void) -> ASIOError {
    guard(ASE_HW_MALFUNCTION, || {
        // SAFETY: a live Driver.
        let rt = &unsafe { me(this) }.rt;
        if let Some(s) = rt.shm() {
            // SAFETY: plain call.
            let on_worker =
                unsafe { GetCurrentThreadId() } == rt.worker_tid.load(Ordering::Acquire);
            let entered = if on_worker {
                rt.entered_ns.load(Ordering::Acquire)
            } else {
                0
            };
            // Before the ack: whoever sees this block returned sees the
            // bit. Set once; later blocks only read it (dasio#28 E4).
            s.header().note_use(HostUse::OutputReady);
            s.header()
                .ack_block(rt.seq_in_hand.load(Ordering::Acquire), entered);
            s.signal_ready();
            rt.ready_sent.store(true, Ordering::Release);
        }
        ASE_OK
    })
}

/// What the worker thread runs on.
struct Job {
    callbacks: ASIOCallbacks,
    time_info: bool,
    shape: Shape,
    instance: u32,
    rt: Arc<Rt>,
    /// This worker's own stop flag.
    stop: Arc<AtomicBool>,
    /// This worker's generation in `Rt::running`.
    generation: u64,
    /// The device's seq when `start` was called: the first block handed
    /// over is the next one (read on the host's thread, so a block published
    /// right after `start` is never missed).
    seen: u64,
}

/// Clears `running` when the worker ends, by a return (a reset asked or a
/// stop) or a panic: its position stops advancing. Only its own generation:
/// an abandoned worker that returns late never clears a newer worker's.
struct Ended<'a>(&'a Job);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        let g = self.0.generation;
        // Err: `stop_worker` or a newer worker already holds the value.
        let _ = self
            .0
            .rt
            .running
            .compare_exchange(g, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl Job {
    fn run(self) {
        let _ended = Ended(&self);
        self.serve();
    }

    /// The worker's loop: blocks to the DAW until stopped, or until it asks
    /// the DAW for a reset.
    fn serve(&self) {
        // SAFETY: plain call.
        self.rt
            .worker_tid
            .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        pro_audio();
        let shm = self.rt.shm();
        let mut time = ASIOTime {
            reserved: [0; 4],
            timeInfo: AsioTimeInfo {
                speed: 1.0,
                systemTime: Asio64::default(),
                samplePosition: Asio64::default(),
                sampleRate: RATE,
                flags: K_SYSTEM_TIME_VALID | K_SAMPLE_POSITION_VALID | K_SAMPLE_RATE_VALID,
                reserved: [0; 12],
            },
            timeCode: ASIOTimeCode {
                speed: 0.0,
                timeCodeSamples: Asio64::default(),
                flags: 0,
                future: [0; 64],
            },
        };
        let period = Duration::from_secs_f64(f64::from(self.shape.block) / RATE);
        let mut next = Instant::now();
        let mut half = 0i32;
        let mut checked = Instant::now();
        let mut watch = shm
            .as_ref()
            .map(|s| Watch::new(self.shape, facts(s), Instant::now()));
        let mut last = self.seen;
        let mut timeline = crate::timeline::Timeline::new();
        while !self.stop.load(Ordering::Acquire) {
            match (shm.as_ref(), watch.as_mut()) {
                (Some(s), Some(watch)) => {
                    if self.check(s, watch, &mut checked) {
                        return;
                    }
                    // A host may stop us from inside a message the check
                    // sent (dasio#28 E16): no block after that.
                    if self.stop.load(Ordering::Acquire) {
                        return;
                    }
                    // The next block (see next_seq). Past QUIET without one,
                    // the device is quiet: the DAW keeps its clock on silence.
                    let Some(seq) = self.next_seq(s, last) else {
                        half ^= 1;
                        self.silence(s, half as u32);
                        self.switch(&mut time, half);
                        continue;
                    };
                    if seq > last + 1 {
                        s.header()
                            .missed
                            .fetch_add(seq - last - 1, Ordering::Relaxed);
                    }
                    // The DAW's timeline runs on the device's media time
                    // (see Timeline): what it measures keeps the latencies
                    // it was told.
                    let start = s.header().start[(seq % 2) as usize].load(Ordering::Relaxed);
                    let pos = timeline.next(
                        (start != shm::NO_START).then_some(start),
                        seq.saturating_sub(last),
                        self.rt.position.load(Ordering::Acquire),
                        u64::from(self.shape.block),
                    );
                    self.rt.position.store(pos, Ordering::Release);
                    last = seq;
                    half = (seq % 2) as i32;
                    self.rt.seq_in_hand.store(seq, Ordering::Release);
                    self.rt.ready_sent.store(false, Ordering::Release);
                    // When the DAW's callback starts on this block: it goes
                    // with the block's ack, and the device splits the
                    // turnaround into our wake-up and the DAW's processing
                    // (dasio#22).
                    self.rt.entered_ns.store(shm::qpc_ns(), Ordering::Release);
                    self.switch(&mut time, half);
                    if !self.rt.ready_sent.swap(false, Ordering::AcqRel) {
                        s.header()
                            .ack_block(seq, self.rt.entered_ns.load(Ordering::Acquire));
                        s.signal_ready();
                    }
                }
                _ => {
                    // Free-run on our own clock; a device that appears asks
                    // the DAW for a reset (it then opens the real buffers).
                    next += period;
                    let now = Instant::now();
                    if next > now {
                        sleep_until(next);
                    } else if now - next > Duration::from_millis(100) {
                        next = now;
                    }
                    half ^= 1;
                    self.switch(&mut time, half);
                    if checked.elapsed() > Duration::from_secs(1) {
                        checked = Instant::now();
                        if Shared::open(self.instance).is_ok() && self.ask_reset(checked) {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Once every [`CHECK_EVERY`]: the device's verdict, kept for
    /// `getErrorMessage`; `true` when the worker ends because a reshaped
    /// device got the DAW a reset request.
    fn check(&self, s: &Shared, watch: &mut Watch, checked: &mut Instant) -> bool {
        let now = Instant::now();
        if now.duration_since(*checked) < CHECK_EVERY {
            return false;
        }
        *checked = now;
        // A device that restarted with another shape made a fresh section:
        // this one is dead, and only a reset (a new driver object) reaches
        // the new one.
        if s.published_session() != s.session() {
            self.rt.restarts.fetch_add(1, Ordering::Relaxed);
            self.rt
                .verdict
                .store(Verdict::Reshaped as u8 + 1, Ordering::Release);
            return self.ask_reset(now);
        }
        let v = watch.observe(facts(s), now);
        self.rt.verdict.store(v as u8 + 1, Ordering::Release);
        if v == Verdict::Restarted {
            self.rt.restarts.fetch_add(1, Ordering::Relaxed);
        }
        match v {
            Verdict::Reshaped => self.ask_reset(now),
            // getLatencies reads the header: a DAW that takes
            // kAsioLatenciesChanged asks it again; one that does not gets a
            // reset (within the budget).
            Verdict::Relatency if self.host_supports(K_ASIO_LATENCIES_CHANGED) => {
                self.message(K_ASIO_LATENCIES_CHANGED);
                false
            }
            Verdict::Relatency => self.ask_reset(now),
            _ => false,
        }
    }

    /// Whether the DAW answers `selector` (kAsioSelectorSupported).
    fn host_supports(&self, selector: i32) -> bool {
        self.callbacks.asioMessage.is_some_and(|m| {
            // SAFETY: the host's callback; the value is the selector asked.
            unsafe {
                m(
                    K_ASIO_SELECTOR_SUPPORTED,
                    selector,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ) != 0
            }
        })
    }

    /// Asks the DAW for a reset if the instance's budget allows; whether it
    /// did.
    fn ask_reset(&self, now: Instant) -> bool {
        let mut budgets = lock(&BUDGETS);
        let allowed = (self.instance as usize)
            .checked_sub(1)
            .and_then(|i| budgets.get_mut(i))
            .is_some_and(|b| b.take(now));
        drop(budgets);
        if allowed {
            self.message(K_ASIO_RESET_REQUEST);
        }
        allowed
    }

    /// The device's next published block after `last`: a short spin on the
    /// header's seq, then a sleep on the block event (a sleeping thread is
    /// woken at once and boosted; a spinning one shares its cores' quanta
    /// with the DAW's other audio threads: 30 ms stalls on the FOH PC);
    /// `None` after [`QUIET`] without one, or when asked to stop.
    fn next_seq(&self, s: &Shared, last: u64) -> Option<u64> {
        let until = Instant::now() + QUIET;
        for _ in 0..SPIN_ROUNDS {
            let seq = s.header().seq.load(Ordering::Acquire);
            if seq != last {
                return Some(seq);
            }
            std::hint::spin_loop();
        }
        loop {
            let seq = s.header().seq.load(Ordering::Acquire);
            if seq != last {
                return Some(seq);
            }
            let now = Instant::now();
            if self.stop.load(Ordering::Acquire) || now >= until {
                return None;
            }
            // Auto-reset: a signal before this wait is not lost; one for an
            // older block only wakes the check above.
            s.wait_block(((until - now).as_millis() as u32).max(1));
        }
    }

    /// Zeroes the inputs of `half` (a device gone quiet).
    fn silence(&self, s: &Shared, half: u32) {
        for ch in 0..self.shape.rx {
            if let Some(p) = s.buffer(half, true, ch) {
                // SAFETY: a buffer of `block` samples in the mapping; the
                // device is not writing it (it went quiet).
                unsafe { core::ptr::write_bytes(p, 0, self.shape.block as usize) };
            }
        }
    }

    /// Sends the DAW `selector`; a reset request or a latency change counts
    /// in the header first (dasio#28 E16: the device logs them), so one the
    /// DAW hangs on is counted too.
    fn message(&self, selector: i32) {
        if let Some(m) = self.callbacks.asioMessage {
            let kind = match selector {
                K_ASIO_RESET_REQUEST => Some(DriverMessage::Reset),
                K_ASIO_LATENCIES_CHANGED => Some(DriverMessage::Latencies),
                _ => None,
            };
            if let (Some(k), Some(s)) = (kind, self.rt.shm()) {
                s.header().note_message(k);
            }
            // SAFETY: the host's callback.
            unsafe { m(selector, 0, core::ptr::null_mut(), core::ptr::null_mut()) };
        }
    }

    /// Hands half `half` to the DAW.
    fn switch(&self, time: &mut ASIOTime, half: i32) {
        let pos = self
            .rt
            .position
            .fetch_add(u64::from(self.shape.block), Ordering::AcqRel);
        let ns = systime::stamp(
            shm::qpc_ns(),
            self.rt.systime_offset.load(Ordering::Acquire),
        );
        self.rt.systime_ns.store(ns, Ordering::Release);
        time.timeInfo.samplePosition = Asio64::new(pos);
        time.timeInfo.systemTime = Asio64::new(ns);
        if self.time_info
            && let Some(f) = self.callbacks.bufferSwitchTimeInfo
        {
            // SAFETY: the host's callback with a valid ASIOTime.
            unsafe { f(time, half, ASIO_TRUE) };
            return;
        }
        if let Some(f) = self.callbacks.bufferSwitch {
            // SAFETY: the host's callback.
            unsafe { f(half, ASIO_TRUE) };
        }
    }
}

/// What the header shows of the device now.
fn facts(s: &Shared) -> Facts {
    let h = s.header();
    Facts {
        beat: h.heartbeat_ms.load(Ordering::Acquire),
        pid: h.device_pid.load(Ordering::Acquire),
        shape: Shape::new(
            h.rx.load(Ordering::Acquire),
            h.tx.load(Ordering::Acquire),
            h.block.load(Ordering::Acquire),
        ),
        latency: (
            h.latency_in.load(Ordering::Acquire),
            h.latency_out.load(Ordering::Acquire),
        ),
    }
}

/// The device's verdict for `getErrorMessage` (empty while live).
fn device_text(verdict: u8, restarts: u32) -> String {
    let state = match verdict {
        v if v == Verdict::Stale as u8 + 1 => "the device stopped (no heartbeat)",
        v if v == Verdict::Reshaped as u8 + 1 => "the device changed its block or channels",
        _ => "",
    };
    match (state.is_empty(), restarts) {
        (true, 0) => String::new(),
        (true, n) => format!("the device restarted {n} times"),
        (false, 0) => state.to_string(),
        (false, n) => format!("{state}; it restarted {n} times"),
    }
}

/// The calling thread into MMCSS "Pro Audio" at critical priority (the
/// ASIO SDK's rule for a driver's audio thread).
fn pro_audio() {
    let task: Vec<u16> = "Pro Audio".encode_utf16().chain(Some(0)).collect();
    let mut index = 0u32;
    // SAFETY: a NUL-terminated name and a writable index; the registration
    // is kept for the thread's life.
    let h = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
    // SAFETY: the registration just made.
    let mmcss = !h.is_null() && unsafe { AvSetMmThreadPriority(h, AVRT_PRIORITY_CRITICAL) } != 0;
    if !mmcss {
        // Without MMCSS the thread still runs above the DAW's own threads.
        // SAFETY: the calling thread's pseudo handle.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    }
}

/// Sleeps until `t`: whole milliseconds by sleep, the rest spun.
fn sleep_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}
