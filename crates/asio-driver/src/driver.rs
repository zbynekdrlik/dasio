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
//! - Once a second the worker ([`worker`]) reads the device's state from
//!   the header ([`Watch`](crate::liveness::Watch)): a device back with
//!   another shape gets the DAW a `kAsioResetRequest`, within the
//!   instance's [`Budget`](crate::reset::Budget); blocks the worker skipped
//!   are counted in the header's `missed`.

use core::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use shm::{HostUse, Shape, Shared};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use crate::abi::*;
use crate::com::OBJECTS;
use crate::guard::{guard, join_within, lock};
use crate::ident::{driver_name, instance_of};
use crate::liveness::Verdict;
use crate::systime;

mod worker;
use worker::Job;

/// The rate: Dante at 96 kHz.
pub const RATE: f64 = 96_000.0;
/// The shape a DAW sees while no device runs (it asks again after a reset).
pub const NO_DEVICE_SHAPE: Shape = Shape {
    rx: 8,
    tx: 8,
    block: 32,
};
/// The longest a DAW's thread waits for the worker to end; past it the
/// worker (stuck in the DAW's callback) is abandoned.
pub const STOP_WAIT: Duration = Duration::from_secs(1);

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
