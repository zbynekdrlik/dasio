//! The worker thread of one instance's driver: it hands each block to the
//! DAW's callbacks (see the parent module).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shm::{DriverMessage, Shape, Shared};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED};
use windows_sys::Win32::System::Threading::{
    AVRT_PRIORITY_CRITICAL, AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread,
    GetCurrentThreadId, SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_TIME_CRITICAL,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};

use super::{RATE, Rt};
use crate::abi::*;
use crate::guard::lock;
use crate::ident::MAX_INSTANCES;
use crate::liveness::{Facts, Verdict, Watch};
use crate::reset::Budget;
use crate::systime;

/// Spin rounds before the worker sleeps on the block event (a few µs).
const SPIN_ROUNDS: u32 = 2_000;
/// A device silent this long is treated as gone (free-run).
const QUIET: Duration = Duration::from_millis(50);
/// How far the no-device free-run may fall behind before it restarts its
/// clock from now (shorter lags are caught up).
const FREE_RUN_LAG: Duration = Duration::from_millis(100);
/// How often the worker reads the device's state.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// The last stretch before a free-run tick, spun instead of waited (the
/// high-resolution timer can wake this much early or late).
const SPIN_RESIDUE: Duration = Duration::from_micros(100);
/// How long past its due time a timer wait gives up (a timer that never
/// fires must not hang the worker; the loop then looks at the clock).
const TIMER_GRACE_MS: u32 = 5;

/// Each instance's reset requests in this process (a host makes a new
/// driver object after a reset, so the budget outlives the object).
static BUDGETS: Mutex<[Budget; MAX_INSTANCES as usize]> =
    Mutex::new([const { Budget::new() }; MAX_INSTANCES as usize]);

/// What the worker thread runs on.
pub(super) struct Job {
    pub(super) callbacks: ASIOCallbacks,
    pub(super) time_info: bool,
    pub(super) shape: Shape,
    pub(super) instance: u32,
    pub(super) rt: Arc<Rt>,
    /// This worker's own stop flag.
    pub(super) stop: Arc<AtomicBool>,
    /// This worker's generation in `Rt::running`.
    pub(super) generation: u64,
    /// The device's seq when `start` was called: the first block handed
    /// over is the next one (read on the host's thread, so a block published
    /// right after `start` is never missed).
    pub(super) seen: u64,
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
    pub(super) fn run(self) {
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
        let mut pacer = Pacer::new();
        let mut next = Instant::now();
        let mut half = 0i32;
        let mut checked = Instant::now();
        let mut watch = shm
            .as_ref()
            .map(|s| Watch::new(self.shape, facts(s), Instant::now()));
        let mut last = self.seen;
        // The free-run clock of a quiet device: the tick of the last silent
        // block handed; `None` while the device delivers.
        let mut quiet: Option<Instant> = None;
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
                    // the device is quiet: the DAW keeps its clock on
                    // silence, one block per period on our own clock (the
                    // host is called at the buffer rate, dasio#28), until a
                    // block arrives again. The first QUIET is lost once per
                    // outage: no catch-up burst.
                    let until = match quiet.as_mut() {
                        Some(tick) => {
                            // A tick missed (a DAW callback that ran
                            // long) is dropped, never caught up.
                            pacer.pace(tick, period, Duration::ZERO);
                            *tick
                        }
                        None => Instant::now() + QUIET,
                    };
                    let Some(seq) = self.next_seq(s, last, until) else {
                        // A stop asked during the wait: no block after it.
                        if self.stop.load(Ordering::Acquire) {
                            return;
                        }
                        quiet.get_or_insert_with(Instant::now);
                        half ^= 1;
                        self.silence(s, half as u32);
                        self.switch(&mut time, half);
                        continue;
                    };
                    quiet = None;
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
                    pacer.pace(&mut next, period, FREE_RUN_LAG);
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
    /// `None` at `until` without one, or when asked to stop. An `until`
    /// already passed checks once (the spin) without sleeping.
    fn next_seq(&self, s: &Shared, last: u64, until: Instant) -> Option<u64> {
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

/// The free-run's clock on the worker thread: it waits for each tick on a
/// high-resolution waitable timer, so a period under the scheduler's tick
/// (64 samples at 96 kHz = 0.67 ms) does not keep a core busy (dasio#87).
/// Without the timer (it could not be made, or a wait failed) it sleeps
/// whole milliseconds and spins the rest.
struct Pacer {
    /// The timer; null without one.
    timer: HANDLE,
}

impl Pacer {
    fn new() -> Self {
        // SAFETY: plain call: no security attributes, no name.
        let timer = unsafe {
            CreateWaitableTimerExW(
                core::ptr::null(),
                core::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        };
        Self { timer }
    }

    /// The free-run clock's next tick: `next` moves on by `period` and the
    /// thread sleeps until it; a clock more than `max_lag` behind (a stalled
    /// thread) restarts from now instead of catching up in a burst.
    fn pace(&mut self, next: &mut Instant, period: Duration, max_lag: Duration) {
        *next += period;
        let now = Instant::now();
        if *next > now {
            self.sleep_until(*next);
        } else if now - *next > max_lag {
            *next = now;
        }
    }

    /// Sleeps until `t`: on the timer up to the last [`SPIN_RESIDUE`], which
    /// is spun; without the timer whole milliseconds by sleep, the rest spun.
    fn sleep_until(&mut self, t: Instant) {
        loop {
            let now = Instant::now();
            if now >= t {
                return;
            }
            let left = t - now;
            if !self.timer.is_null() {
                if left > SPIN_RESIDUE {
                    self.wait(left - SPIN_RESIDUE);
                } else {
                    std::hint::spin_loop();
                }
            } else if left > Duration::from_millis(2) {
                std::thread::sleep(left - Duration::from_millis(1));
            } else {
                std::hint::spin_loop();
            }
        }
    }

    /// Waits `d` on the timer. A timer that cannot be armed or waited is
    /// closed: the pacer goes on without it.
    fn wait(&mut self, d: Duration) {
        // A relative due time: negative, in 100 ns units (at least one).
        let ticks = i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX).max(1);
        let due = -ticks;
        // Bounded: a period is milliseconds, and never INFINITE.
        let ms = u32::try_from(d.as_millis()).unwrap_or(u32::MAX).min(60_000) + TIMER_GRACE_MS;
        // SAFETY: our timer and a due time that outlives the call; no
        // completion routine, no resume.
        let armed =
            unsafe { SetWaitableTimer(self.timer, &due, 0, None, core::ptr::null(), 0) } != 0;
        // SAFETY: our timer, armed above.
        if !armed || unsafe { WaitForSingleObject(self.timer, ms) } == WAIT_FAILED {
            self.close();
        }
    }

    /// Closes the timer once; the pacer then sleeps and spins.
    fn close(&mut self) {
        if !self.timer.is_null() {
            // SAFETY: the timer this pacer made, closed once.
            unsafe { CloseHandle(self.timer) };
            self.timer = core::ptr::null_mut();
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        self.close();
    }
}
