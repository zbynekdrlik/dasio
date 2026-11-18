//! The process keeper's Windows glue (dasio#185): one pass over the
//! process list, setting what [`keep::wanted`](super::super::keep::wanted)
//! says. Toolhelp and process handles only: no child process. I/O, memory
//! and GPU priority and the boost are [`extras`](super::extras).

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask,
};

use super::super::keep::{Fights, Note, Set, gave_up_line, held, line, mask_outside, wanted};
use super::super::{ProcEntry, ProcState, Profile};
use super::{extras, from_wide};

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

/// A class's priority name.
fn name(class: u32) -> &'static str {
    match class {
        IDLE_PRIORITY_CLASS => "idle",
        BELOW_NORMAL_PRIORITY_CLASS => "below normal",
        NORMAL_PRIORITY_CLASS => "normal",
        ABOVE_NORMAL_PRIORITY_CLASS => "above normal",
        HIGH_PRIORITY_CLASS => "high",
        REALTIME_PRIORITY_CLASS => "realtime",
        _ => "unknown",
    }
}

/// One process, opened once: its image checked, read and set through the
/// same handle, so a process id reused meanwhile is never touched (an
/// open handle keeps its id from being reused).
struct Proc(HANDLE);

impl Drop for Proc {
    fn drop(&mut self) {
        // SAFETY: the handle `open` returned, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Why a process was not opened.
enum NotOpened {
    /// Access denied: a protected process, said once.
    Denied,
    /// Gone meanwhile: nothing to keep.
    Gone,
}

impl Proc {
    fn open(pid: u32) -> Result<Proc, NotOpened> {
        // SAFETY: plain arguments; the handle is closed by Drop.
        let h = unsafe {
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if !h.is_null() {
            return Ok(Proc(h));
        }
        // SAFETY: no arguments; the open's error.
        if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
            Err(NotOpened::Denied)
        } else {
            Err(NotOpened::Gone)
        }
    }

    /// Whether its image's file name is `exe` (case ignored).
    fn image_is(&self, exe: &str) -> bool {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: a valid handle and a buffer of `len` chars.
        let ok = unsafe {
            QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        } != 0;
        let path = from_wide(&buf[..len as usize]);
        ok && path
            .rsplit('\\')
            .next()
            .is_some_and(|f| f.eq_ignore_ascii_case(exe))
    }

    /// Its affinity and priority, and what else `e` names through `x`
    /// (its own handle), with the PC's CPUs.
    fn state(&self, x: Option<&extras::Handle>, e: &ProcEntry) -> Option<(ProcState, u64)> {
        let (mut mask, mut system) = (0usize, 0usize);
        // SAFETY: a valid handle and two writable masks.
        let ok = unsafe { GetProcessAffinityMask(self.0, &mut mask, &mut system) } != 0;
        // SAFETY: a valid handle.
        let class = unsafe { GetPriorityClass(self.0) };
        (ok && class != 0).then(|| {
            (
                ProcState {
                    affinity: mask as u64,
                    priority: name(class).to_string(),
                    integrity: Err("not read by the keeper".into()),
                    extra: extras::read(x, e),
                },
                system as u64,
            )
        })
    }

    /// Sets `set`: the affinity first (a mask naming a CPU the PC does not
    /// have is refused), the priority only after it, read back (realtime
    /// without the privilege becomes high), then I/O, memory and GPU
    /// priority and the boost through `x`, each read back. Returns what it
    /// set, and why the rest was not.
    fn set(&self, x: Option<&extras::Handle>, set: &Set, system: u64) -> (Set, Option<String>) {
        let mut done = Set::default();
        if let Some(m) = set.affinity {
            if let Some(why) = mask_outside(m, system) {
                return (done, Some(why));
            }
            // SAFETY: a valid handle with PROCESS_SET_INFORMATION.
            if unsafe { SetProcessAffinityMask(self.0, m as usize) } == 0 {
                let e = std::io::Error::last_os_error();
                return (done, Some(format!("affinity 0x{m:X}: {e}")));
            }
            done.affinity = Some(m);
        }
        if let Some(p) = &set.priority {
            let c = match class(p) {
                Ok(c) => c,
                Err(e) => return (done, Some(e)),
            };
            // SAFETY: as above.
            if unsafe { SetPriorityClass(self.0, c) } == 0 {
                let e = std::io::Error::last_os_error();
                return (done, Some(format!("priority: {e}")));
            }
            // SAFETY: as above.
            let now = unsafe { GetPriorityClass(self.0) };
            if now != c {
                let why = format!("priority {} reads back as {}", name(c), name(now));
                return (done, Some(why));
            }
            done.priority = Some(p.clone());
        }
        let err = extras::set(x, set, &mut done);
        (done, err)
    }
}

/// One pass at `now_ms`: every held entry's running processes get what
/// differs. Returns the log lines: each change (also a partial one beside
/// a failure), each process given up ([`Fights`]) and each new failure
/// (once per process and error in `failed`; a list error once until a
/// list read works).
pub fn pass(
    p: &Profile,
    fights: &mut Fights,
    failed: &mut std::collections::HashSet<(u32, String)>,
    now_ms: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    let procs = match super::snapshot() {
        Ok(procs) => procs,
        Err(e) => {
            // Process id 0 is never a held one: the list error's key.
            if failed.insert((0, e.clone())) {
                lines.push(e);
            }
            return lines;
        }
    };
    let mut alive = Vec::new();
    for e in held(p) {
        for (pid, _) in procs.iter().filter(|(_, n)| n.eq_ignore_ascii_case(&e.exe)) {
            let pid = *pid;
            alive.push(pid);
            if fights.gave_up(pid) {
                continue;
            }
            let h = match Proc::open(pid) {
                Ok(h) => h,
                Err(NotOpened::Gone) => continue,
                Err(NotOpened::Denied) => {
                    let why = "cannot open it to set it (access denied: a protected process)";
                    if failed.insert((pid, why.to_string())) {
                        lines.push(format!("could not keep {} (pid {pid}): {why}", e.exe));
                    }
                    continue;
                }
            };
            if !h.image_is(&e.exe) {
                continue;
            }
            // The pid stays this process while `h` is open.
            let x = extras::open(pid, e, true);
            let Some((s, system)) = h.state(x.as_ref(), e) else {
                continue;
            };
            let Some(set) = wanted(e, &s) else { continue };
            let (done, err) = h.set(x.as_ref(), &set, system);
            // What was set is logged and counted, also beside a failure.
            if !done.is_empty() {
                lines.push(line(&e.exe, pid, &s, &done));
                if fights.note(pid, now_ms) == Note::GiveUp {
                    lines.push(gave_up_line(&e.exe, pid));
                }
            }
            if let Some(err) = err
                && failed.insert((pid, err.clone()))
            {
                lines.push(format!("could not keep {} (pid {pid}): {err}", e.exe));
            }
        }
    }
    fights.keep_only(&alive);
    failed.retain(|(pid, _)| alive.contains(pid));
    lines
}
