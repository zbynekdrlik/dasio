//! The Windows gatherer of [`super::Sample`]: every process's CPU time
//! (Toolhelp's process list, `GetProcessTimes` on a limited-query handle)
//! and this process's threads' (Toolhelp's thread list, `GetThreadTimes`,
//! `GetThreadDescription`). Documented calls only; a process or thread
//! that cannot be opened (protected, or gone meanwhile) is left out.

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, GetThreadDescription, GetThreadTimes, OpenProcess,
    OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
};

use super::{Proc, Sample, Thread};

/// Kernel plus user time of two FILETIMEs, 100 ns units.
fn cpu(kernel: &FILETIME, user: &FILETIME) -> u64 {
    super::filetime(kernel.dwHighDateTime, kernel.dwLowDateTime)
        .saturating_add(super::filetime(user.dwHighDateTime, user.dwLowDateTime))
}

/// `h`'s description (`SetThreadDescription`), empty without one.
///
/// # Safety
///
/// `h` is an open thread handle with THREAD_QUERY_LIMITED_INFORMATION.
pub unsafe fn thread_name(h: HANDLE) -> String {
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: a valid handle; Windows allocates the name, freed below.
    if unsafe { GetThreadDescription(h, &mut text) } < 0 || text.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated string from Windows.
    let n = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `n` chars before the NUL.
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, n) });
    // SAFETY: allocated by GetThreadDescription.
    unsafe { LocalFree(text.cast()) };
    s
}

/// How often the process and thread lists are read again (new processes,
/// ended ones, our new threads). Between, a sample only reads the times of
/// the handles already open: Toolhelp's lists cost tens of ms on a busy PC
/// (54 ms on a CI runner, 153 processes), the times microseconds.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// Ids and names from a Toolhelp list (a thread's name is read once it is
/// open).
type Ids = Vec<(u32, String)>;

/// An open handle and what it is.
struct Open {
    id: u32,
    name: String,
    h: HANDLE,
}

/// Kernel plus user time from `times` (a GetProcessTimes or GetThreadTimes
/// call on four FILETIMEs), `None` once it fails (the process or thread
/// ended).
fn cpu_of(times: impl FnOnce(&mut [FILETIME; 4]) -> windows_sys::core::BOOL) -> Option<u64> {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut t = [z; 4];
    (times(&mut t) != 0).then(|| cpu(&t[2], &t[3]))
}

/// Every process (limited-query handles), this process's threads and the
/// DAW's, kept open between samples and read again every
/// [`REFRESH_EVERY`].
pub struct Gatherer {
    procs: Vec<Open>,
    threads: Vec<Open>,
    daw: Vec<Open>,
    /// The DAW's process id (0: none): the driver's word in the shared
    /// directory, read at each refresh.
    host_pid: Box<dyn Fn() -> u32 + Send>,
    refreshed: Option<std::time::Instant>,
    /// The last refresh's cost, µs.
    pub refresh_us: u64,
}

impl Gatherer {
    /// A gatherer that asks `host_pid` for the DAW's process at each
    /// refresh (0: no DAW).
    pub fn new(host_pid: impl Fn() -> u32 + Send + 'static) -> Self {
        Gatherer {
            procs: Vec::new(),
            threads: Vec::new(),
            daw: Vec::new(),
            host_pid: Box::new(host_pid),
            refreshed: None,
            refresh_us: 0,
        }
    }

    /// Opens what is new in the process list and our thread list; keeps
    /// what stays (same id and name), closes what ended.
    fn refresh(&mut self) {
        let t = std::time::Instant::now();
        let procs = list_procs();
        keep_and_open(&mut self.procs, procs, |id| {
            // SAFETY: a limited-query handle to a process, closed by
            // `close` or Drop.
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id) }
        });
        // SAFETY: no arguments.
        let own = unsafe { GetCurrentProcessId() };
        let (threads, daw) = list_threads(own, (self.host_pid)());
        let open = |id| {
            // SAFETY: a limited-query handle to a thread, closed by
            // `keep_and_open` or Drop.
            unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, id) }
        };
        keep_and_open(&mut self.threads, threads, open);
        keep_and_open(&mut self.daw, daw, open);
        for o in self.threads.iter_mut().chain(self.daw.iter_mut()) {
            if o.name.is_empty() {
                // SAFETY: opened above with limited query rights.
                o.name = unsafe { thread_name(o.h) };
            }
        }
        self.refreshed = Some(std::time::Instant::now());
        self.refresh_us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
    }

    /// A sample now, `at_ms` on the caller's steady clock; the lists are
    /// read again first when [`REFRESH_EVERY`] has passed.
    pub fn sample(&mut self, at_ms: u64) -> Sample {
        if self.refreshed.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
            self.refresh();
        }
        Sample {
            at_ms,
            procs: self
                .procs
                .iter()
                .filter_map(|o| {
                    cpu_of(|[c, x, k, u]| {
                        // SAFETY: an open handle and four writable FILETIMEs.
                        unsafe { GetProcessTimes(o.h, c, x, k, u) }
                    })
                    .map(|cpu_100ns| Proc {
                        pid: o.id,
                        name: o.name.clone(),
                        cpu_100ns,
                    })
                })
                .collect(),
            threads: thread_times(&self.threads),
            daw: thread_times(&self.daw),
        }
    }
}

/// The CPU times of `open` threads; one that ended is left out.
fn thread_times(open: &[Open]) -> Vec<Thread> {
    open.iter()
        .filter_map(|o| {
            cpu_of(|[c, x, k, u]| {
                // SAFETY: an open handle and four writable FILETIMEs.
                unsafe { GetThreadTimes(o.h, c, x, k, u) }
            })
            .map(|cpu_100ns| Thread {
                tid: o.id,
                name: o.name.clone(),
                cpu_100ns,
            })
        })
        .collect()
}

impl Drop for Gatherer {
    fn drop(&mut self) {
        for o in self
            .procs
            .drain(..)
            .chain(self.threads.drain(..))
            .chain(self.daw.drain(..))
        {
            // SAFETY: opened by `refresh`, closed once here.
            unsafe { CloseHandle(o.h) };
        }
    }
}

/// `open` becomes `now` (ids and names): an entry in both keeps its handle,
/// one only in `open` is closed, one only in `now` is opened with `opener`
/// (left out when that fails).
fn keep_and_open(open: &mut Vec<Open>, now: Ids, opener: impl Fn(u32) -> HANDLE) {
    let mut old = std::mem::take(open);
    for (id, name) in now {
        if let Some(i) = old
            .iter()
            .position(|o| o.id == id && (o.name == name || name.is_empty()))
        {
            open.push(old.swap_remove(i));
            continue;
        }
        let h = opener(id);
        if !h.is_null() {
            open.push(Open { id, name, h });
        }
    }
    for o in old {
        // SAFETY: opened by an earlier refresh, closed once here.
        unsafe { CloseHandle(o.h) };
    }
}

/// Every process's id and exe name (Toolhelp).
fn list_procs() -> Ids {
    let mut out = Vec::new();
    // SAFETY: a process snapshot, closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    let mut e = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: a valid snapshot and an entry with its size set.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        out.push((e.th32ProcessID, super::utf16_name(&e.szExeFile)));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot from above.
    unsafe { CloseHandle(snap) };
    out
}

/// The thread ids of process `own` and of process `daw` (0: none), from
/// one Toolhelp list; names are read once opened.
fn list_threads(own: u32, daw: u32) -> (Ids, Ids) {
    let (mut mine, mut theirs) = (Vec::new(), Vec::new());
    // SAFETY: a thread snapshot, closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return (mine, theirs);
    }
    // SAFETY: a zeroed entry with its size set, as the API wants.
    let mut e: THREADENTRY32 = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    // SAFETY: a valid snapshot and entry.
    let mut ok = unsafe { Thread32First(snap, &mut e) } != 0;
    while ok {
        // Both lists for one process when the DAW is this one (a test).
        if e.th32OwnerProcessID == own {
            mine.push((e.th32ThreadID, String::new()));
        }
        if daw != 0 && e.th32OwnerProcessID == daw {
            theirs.push((e.th32ThreadID, String::new()));
        }
        // SAFETY: as above.
        ok = unsafe { Thread32Next(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot from above.
    unsafe { CloseHandle(snap) };
    (mine, theirs)
}

/// How often a sample is taken while no burst asks for one.
const EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How often the samples' own cost is logged.
const COST_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// The `cpu-snap` thread: a sample every [`EVERY`], and one at once for
/// each `()` the returned sender gets (the run loop sends one at a burst
/// start), logged against the sample before it. The thread ends when the
/// sender is dropped. The helper placer keeps it on the helper CPUs.
pub fn spawn(
    start: std::time::Instant,
    host_pid: impl Fn() -> u32 + Send + 'static,
) -> std::io::Result<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("cpu-snap".into())
        .spawn(move || run(start, Gatherer::new(host_pid), &rx))?;
    Ok((tx, thread))
}

/// The samples' own cost since it was last logged.
struct Cost {
    gatherer: Gatherer,
    taken: u64,
    sum_us: u64,
    max_us: u64,
}

impl Cost {
    /// A sample at `at` ms, its cost counted; the sample and its µs.
    fn take(&mut self, at: u64) -> (Sample, u64) {
        let t = std::time::Instant::now();
        let s = self.gatherer.sample(at);
        let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.taken += 1;
        self.sum_us = self.sum_us.saturating_add(us);
        self.max_us = self.max_us.max(us);
        (s, us)
    }
}

fn run(start: std::time::Instant, gatherer: Gatherer, rx: &std::sync::mpsc::Receiver<()>) {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;
    let ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut cost = Cost {
        gatherer,
        taken: 0,
        sum_us: 0,
        max_us: 0,
    };
    let (mut prev, us) = cost.take(ms());
    tracing::info!(
        procs = prev.procs.len(),
        threads = prev.threads.len(),
        daw_threads = prev.daw.len(),
        sample_us = us,
        refresh_us = cost.gatherer.refresh_us,
        every_ms = EVERY.as_millis() as u64,
        "CPU samples start: each burst of missed DAW blocks names who ran since the sample before"
    );
    let mut cost_at = Instant::now();
    let mut total = 0u64;
    loop {
        match rx.recv_timeout(EVERY) {
            Ok(()) => {
                let (now, us) = cost.take(ms());
                let procs = super::top_procs(&prev, &now, super::TOP_PROCS);
                let threads = super::top_threads(&prev, &now, super::TOP_THREADS);
                let daw = super::top_daw_threads(&prev, &now, super::TOP_DAW_THREADS);
                tracing::warn!(
                    span_ms = now.at_ms.saturating_sub(prev.at_ms),
                    procs = %super::list(&procs),
                    dasio_threads = %super::list(&threads),
                    daw_threads = %super::list(&daw),
                    sample_us = us,
                    "who ran since the last CPU sample, at the DAW's burst of missed blocks"
                );
                prev = now;
            }
            Err(RecvTimeoutError::Timeout) => prev = cost.take(ms()).0,
            Err(RecvTimeoutError::Disconnected) => {
                tracing::info!(samples = total + cost.taken, "CPU samples end");
                return;
            }
        }
        if cost_at.elapsed() >= COST_EVERY {
            tracing::info!(
                samples = cost.taken,
                mean_us = cost.sum_us / cost.taken.max(1),
                max_us = cost.max_us,
                last_refresh_us = cost.gatherer.refresh_us,
                procs = prev.procs.len(),
                threads = prev.threads.len(),
                daw_threads = prev.daw.len(),
                "CPU samples' own cost in the last 10 min"
            );
            total += cost.taken;
            (cost.taken, cost.sum_us, cost.max_us) = (0, 0, 0);
            cost_at = Instant::now();
        }
    }
}
