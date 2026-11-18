//! The process keeper's Windows glue (dasio#185): one pass over the
//! process list, setting what [`keep::wanted`](super::super::keep::wanted)
//! says. Toolhelp and process handles only: no child process.

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask,
};

use super::super::Profile;
use super::super::keep::{Quiet, Set, counted, held, line, wanted};

/// A priority name's class.
fn class(name: &str) -> Result<u32, String> {
    Ok(match name {
        "idle" => IDLE_PRIORITY_CLASS,
        "below normal" => BELOW_NORMAL_PRIORITY_CLASS,
        "normal" => NORMAL_PRIORITY_CLASS,
        "above normal" => ABOVE_NORMAL_PRIORITY_CLASS,
        "high" => HIGH_PRIORITY_CLASS,
        "realtime" => REALTIME_PRIORITY_CLASS,
        other => return Err(format!("priority {other:?} is not a class")),
    })
}

/// Sets `set` on process `pid`.
pub fn set_on(pid: u32, set: &Set) -> Result<(), String> {
    let class = set.priority.as_deref().map(class).transpose()?;
    // SAFETY: a set-information handle, closed below.
    let h = unsafe {
        OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if h.is_null() {
        return Err(format!("open: {}", std::io::Error::last_os_error()));
    }
    let mut out = Ok(());
    // SAFETY: a valid handle with PROCESS_SET_INFORMATION.
    if let Some(c) = class
        && unsafe { SetPriorityClass(h, c) } == 0
    {
        out = Err(format!("priority: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: as above; a mask of this PC's CPUs.
    if let Some(m) = set.affinity
        && out.is_ok()
        && unsafe { SetProcessAffinityMask(h, m as usize) } == 0
    {
        out = Err(format!(
            "affinity 0x{m:X}: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the handle opened above, closed once.
    unsafe { CloseHandle(h) };
    out
}

/// One pass at `now_ms`: every held entry's running processes get what
/// differs. Returns the log lines: changes (quieted per process by
/// `quiet`) and each new failure (once per process and error, in
/// `failed`).
pub fn pass(
    p: &Profile,
    quiet: &mut Quiet,
    failed: &mut std::collections::HashSet<(u32, String)>,
    now_ms: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut alive = Vec::new();
    for e in held(p) {
        let pids = match super::find_processes(&e.exe) {
            Ok(pids) => pids,
            Err(err) => {
                lines.push(err);
                return lines;
            }
        };
        for pid in pids {
            alive.push(pid);
            // Gone meanwhile, or a protected process: nothing to keep.
            let Some(s) = super::proc_state(pid) else {
                continue;
            };
            let Some(set) = wanted(e, &s) else {
                continue;
            };
            match set_on(pid, &set) {
                Ok(()) => {
                    if let Some(n) = quiet.note(pid, now_ms) {
                        lines.push(counted(line(&e.exe, pid, &s, &set), n));
                    }
                }
                Err(err) => {
                    if failed.insert((pid, err.clone())) {
                        lines.push(format!("could not keep {} (pid {pid}): {err}", e.exe));
                    }
                }
            }
        }
    }
    quiet.keep_only(&alive);
    failed.retain(|(pid, _)| alive.contains(pid));
    lines
}
