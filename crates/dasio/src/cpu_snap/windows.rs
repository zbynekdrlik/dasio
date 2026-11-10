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
    let t = |f: &FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    t(kernel).saturating_add(t(user))
}

/// A NUL-terminated UTF-16 buffer as a string.
fn utf16(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

/// `h`'s description (`SetThreadDescription`), empty without one. `h` needs
/// THREAD_QUERY_LIMITED_INFORMATION.
pub fn thread_name(h: HANDLE) -> String {
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

/// Every process Windows lets us query, with its CPU time so far.
fn procs() -> Vec<Proc> {
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
        // SAFETY: a limited-query handle to a process, closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID) };
        if !h.is_null() {
            let z = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut c, mut x, mut k, mut u) = (z, z, z, z);
            // SAFETY: a valid handle and four writable FILETIMEs.
            if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                out.push(Proc {
                    pid: e.th32ProcessID,
                    name: utf16(&e.szExeFile),
                    cpu_100ns: cpu(&k, &u),
                });
            }
            // SAFETY: opened above.
            unsafe { CloseHandle(h) };
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot from above.
    unsafe { CloseHandle(snap) };
    out
}

/// This process's threads with their CPU time so far.
fn threads() -> Vec<Thread> {
    let mut out = Vec::new();
    // SAFETY: no arguments.
    let pid = unsafe { GetCurrentProcessId() };
    // SAFETY: a thread snapshot, closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: a zeroed entry with its size set, as the API wants.
    let mut e: THREADENTRY32 = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    // SAFETY: a valid snapshot and entry.
    let mut ok = unsafe { Thread32First(snap, &mut e) } != 0;
    while ok {
        if e.th32OwnerProcessID == pid {
            // SAFETY: a limited-query handle to one of our threads, closed
            // below.
            let h = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, e.th32ThreadID) };
            if !h.is_null() {
                let z = FILETIME {
                    dwLowDateTime: 0,
                    dwHighDateTime: 0,
                };
                let (mut c, mut x, mut k, mut u) = (z, z, z, z);
                // SAFETY: a valid handle and four writable FILETIMEs.
                if unsafe { GetThreadTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0 {
                    out.push(Thread {
                        tid: e.th32ThreadID,
                        name: thread_name(h),
                        cpu_100ns: cpu(&k, &u),
                    });
                }
                // SAFETY: opened above.
                unsafe { CloseHandle(h) };
            }
        }
        // SAFETY: as above.
        ok = unsafe { Thread32Next(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot from above.
    unsafe { CloseHandle(snap) };
    out
}

/// A sample now, `at_ms` on the caller's steady clock.
pub fn sample(at_ms: u64) -> Sample {
    Sample {
        at_ms,
        procs: procs(),
        threads: threads(),
    }
}

/// How often a sample is taken while no burst asks for one.
const EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How often the samples' own cost is logged.
const COST_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// The `cpu-snap` thread: a sample every [`EVERY`], and one at once for
/// each `()` the returned sender gets (the run loop sends one at a burst
/// start), logged against the sample before it. The thread ends when the
/// sender is dropped. The helper placer keeps it on the helper CPUs.
pub fn spawn(start: std::time::Instant) -> std::io::Result<std::sync::mpsc::Sender<()>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("cpu-snap".into())
        .spawn(move || run(start, &rx))?;
    Ok(tx)
}

/// The samples' own cost since it was last logged.
#[derive(Default)]
struct Cost {
    taken: u64,
    sum_us: u64,
    max_us: u64,
}

impl Cost {
    /// A sample at `at` ms, its cost counted; the sample and its µs.
    fn take(&mut self, at: u64) -> (Sample, u64) {
        let t = std::time::Instant::now();
        let s = sample(at);
        let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.taken += 1;
        self.sum_us = self.sum_us.saturating_add(us);
        self.max_us = self.max_us.max(us);
        (s, us)
    }
}

fn run(start: std::time::Instant, rx: &std::sync::mpsc::Receiver<()>) {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;
    let ms = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut cost = Cost::default();
    let (mut prev, us) = cost.take(ms());
    tracing::info!(
        procs = prev.procs.len(),
        threads = prev.threads.len(),
        sample_us = us,
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
                tracing::warn!(
                    span_ms = now.at_ms.saturating_sub(prev.at_ms),
                    procs = %super::list(&procs),
                    dasio_threads = %super::list(&threads),
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
                procs = prev.procs.len(),
                threads = prev.threads.len(),
                "CPU samples' own cost in the last 10 min"
            );
            total += cost.taken;
            cost = Cost::default();
            cost_at = Instant::now();
        }
    }
}
