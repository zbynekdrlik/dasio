//! The process keeper's Windows glue (dasio#185): one pass over the
//! process list, setting what [`keep::wanted`](super::super::keep::wanted)
//! says for each `[[process]]` entry and what
//! [`keep::others_wanted`](super::super::keep::others_wanted) says for the
//! rest (`others_affinity`). Toolhelp and process handles only: no child
//! process. I/O, memory and GPU priority and the boost are
//! [`extras`](super::extras).

use std::collections::HashSet;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, FILETIME, GetLastError, HANDLE,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetActiveProcessorGroupCount,
    GetCurrentProcess, GetPriorityClass, GetProcessAffinityMask, GetProcessTimes,
    HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask,
};

use super::super::keep::{
    Fights, Note, Processes, Seen, Set, age_ms, catch_all_takes, gave_up_line, held, line,
    mask_outside, others, others_line, others_wanted, wanted,
};
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

/// A FILETIME as one count of 100 ns units.
fn ticks(t: &FILETIME) -> u64 {
    crate::cpu_snap::filetime(t.dwHighDateTime, t.dwLowDateTime)
}

/// A zero FILETIME, to be written by a call.
const ZERO: FILETIME = FILETIME {
    dwLowDateTime: 0,
    dwHighDateTime: 0,
};

/// Every CPU of the PC (the system affinity mask, read through this
/// process); `None` when unreadable, or when the PC has more than one
/// processor group: a mask is then relative to a process's group, and the
/// same bits name other CPUs in another group (review of PR #198).
pub fn system_mask() -> Option<u64> {
    // SAFETY: no arguments.
    if unsafe { GetActiveProcessorGroupCount() } != 1 {
        return None;
    }
    let (mut mask, mut system) = (0usize, 0usize);
    // SAFETY: this process's pseudo-handle and two writable masks.
    let ok = unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut mask, &mut system) } != 0;
    ok.then_some(system as u64)
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

    /// Its affinity and the PC's CPUs.
    fn affinity(&self) -> Option<(u64, u64)> {
        let (mut mask, mut system) = (0usize, 0usize);
        // SAFETY: a valid handle and two writable masks.
        let ok = unsafe { GetProcessAffinityMask(self.0, &mut mask, &mut system) } != 0;
        ok.then_some((mask as u64, system as u64))
    }

    /// When it started, in 100 ns units since 1601 (FILETIME).
    fn created(&self) -> Option<u64> {
        let (mut created, mut exited, mut kernel, mut user) = (ZERO, ZERO, ZERO, ZERO);
        // SAFETY: a valid handle (a limited query is enough) and four
        // writable FILETIMEs.
        let ok =
            unsafe { GetProcessTimes(self.0, &mut created, &mut exited, &mut kernel, &mut user) }
                != 0;
        ok.then(|| ticks(&created))
    }

    /// Its affinity and priority, and what else `e` names through `x`
    /// (its own handle), with the PC's CPUs.
    fn state(&self, x: Option<&extras::Handle>, e: &ProcEntry) -> Option<(ProcState, u64)> {
        let (mask, system) = self.affinity()?;
        // SAFETY: a valid handle.
        let class = unsafe { GetPriorityClass(self.0) };
        (class != 0).then(|| {
            (
                ProcState {
                    affinity: mask,
                    priority: name(class).to_string(),
                    integrity: Err("not read by the keeper".into()),
                    extra: extras::read(x, e),
                },
                system,
            )
        })
    }

    /// Sets `set`: the affinity first (a mask naming a CPU the PC does not
    /// have is refused), read back, the priority only after it, read back
    /// (realtime without the privilege becomes high), then I/O, memory and
    /// GPU priority and the boost through `x`, each read back. Returns what
    /// it set, and why the rest was not.
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
            // Set: logged and counted toward a fight also when the read-back
            // differs (review of PR #198).
            done.affinity = Some(m);
            match self.affinity() {
                Some((now, _)) if now == m => {}
                Some((now, _)) => {
                    return (
                        done,
                        Some(format!("affinity 0x{m:X} reads back as 0x{now:X}")),
                    );
                }
                None => return (done, Some(format!("affinity 0x{m:X} does not read back"))),
            }
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

/// One pass's bookkeeping: the lines it logs, the processes it saw (for
/// [`Fights`] and the failures said once), shared by the entries and the
/// catch-all.
struct Pass<'a> {
    fights: &'a mut Fights,
    failed: &'a mut HashSet<(u32, String)>,
    lines: Vec<String>,
    alive: Vec<u32>,
    now_ms: u64,
}

impl Pass<'_> {
    /// `pid`, opened to be kept as `exe`: `None` when the keeper gave it
    /// up, it is gone, it refuses the open (said once when `say_denied`:
    /// an entry's process; the catch-all skips a protected one silently,
    /// as the check does) or its image is another one now (a reused id).
    fn open(&mut self, pid: u32, exe: &str, say_denied: bool) -> Option<Proc> {
        self.alive.push(pid);
        if self.fights.gave_up(pid) {
            return None;
        }
        let h = match Proc::open(pid) {
            Ok(h) => h,
            Err(NotOpened::Gone) => return None,
            Err(NotOpened::Denied) if !say_denied => return None,
            Err(NotOpened::Denied) => {
                let why = "cannot open it to set it (access denied: a protected process)";
                if self.failed.insert((pid, why.to_string())) {
                    self.lines
                        .push(format!("could not keep {exe} (pid {pid}): {why}"));
                }
                return None;
            }
        };
        // The pid stays this process while `h` is open.
        h.image_is(exe).then_some(h)
    }

    /// What was set on `pid` (`changed`, its log line), logged and counted
    /// toward a fight, also beside a failure; a failure logged once per
    /// process and error.
    fn record(&mut self, exe: &str, pid: u32, changed: Option<String>, err: Option<String>) {
        if let Some(l) = changed {
            self.lines.push(l);
            if self.fights.note(pid, self.now_ms) == Note::GiveUp {
                self.lines.push(gave_up_line(exe, pid));
            }
        }
        if let Some(err) = err
            && self.failed.insert((pid, err.clone()))
        {
            self.lines
                .push(format!("could not keep {exe} (pid {pid}): {err}"));
        }
    }
}

/// One pass at `now_ms` over the running processes ([`pass_over`]).
/// Returns the log lines; a list error once until a list read works.
pub fn pass(
    p: &Profile,
    fights: &mut Fights,
    failed: &mut HashSet<(u32, String)>,
    now_ms: u64,
) -> Vec<String> {
    match super::snapshot() {
        Ok(procs) => pass_over(p, &procs, fights, failed, now_ms),
        // Process id 0 is never a held one: the list error's key.
        Err(e) => {
            if failed.insert((0, e.clone())) {
                vec![e]
            } else {
                Vec::new()
            }
        }
    }
}

/// One pass at `now_ms` over `procs` (id and exe file name, as the process
/// list gives them): every held entry's processes get what differs; then,
/// with `others_affinity`, every process the catch-all takes that still
/// runs on every CPU gets that affinity, read back. Returns the log lines:
/// each change (also a partial one beside a failure), each process given
/// up ([`Fights`]) and each new failure (once per process and error in
/// `failed`).
pub fn pass_over(
    p: &Profile,
    procs: &[(u32, String)],
    fights: &mut Fights,
    failed: &mut HashSet<(u32, String)>,
    now_ms: u64,
) -> Vec<String> {
    let mut k = Pass {
        fights,
        failed,
        lines: Vec::new(),
        alive: Vec::new(),
        now_ms,
    };
    for e in held(p) {
        for (pid, _) in procs.iter().filter(|(_, n)| n.eq_ignore_ascii_case(&e.exe)) {
            let Some(h) = k.open(*pid, &e.exe, true) else {
                continue;
            };
            let x = extras::open(*pid, e, true);
            let Some((s, system)) = h.state(x.as_ref(), e) else {
                continue;
            };
            let Some(set) = wanted(e, &s) else { continue };
            let (done, err) = h.set(x.as_ref(), &set, system);
            let changed = (!done.is_empty()).then(|| line(&e.exe, *pid, &s, &done));
            k.record(&e.exe, *pid, changed, err);
        }
    }
    // A mask naming a CPU the PC lacks is said once, at the profile load
    // (`keep::others_loaded`), never once per process.
    if let Some(want) = system_mask().and_then(|s| others(p, s).ok().flatten()) {
        let own = std::process::id();
        for (pid, exe) in procs
            .iter()
            .filter(|(pid, exe)| catch_all_takes(p, exe, *pid, own))
        {
            let Some(h) = k.open(*pid, exe, false) else {
                continue;
            };
            let Some((found, system)) = h.affinity() else {
                continue;
            };
            let Some(m) = others_wanted(want, found, system) else {
                continue;
            };
            let only = Set {
                affinity: Some(m),
                ..Set::default()
            };
            let (done, err) = h.set(None, &only, system);
            let changed = done.affinity.map(|to| others_line(exe, *pid, found, to));
            k.record(exe, *pid, changed, err);
        }
    }
    let Pass {
        fights,
        failed,
        lines,
        alive,
        ..
    } = k;
    fights.keep_only(&alive);
    failed.retain(|(pid, _)| alive.contains(pid));
    lines
}

/// Every running process the keeper could open to set, with the PC's
/// CPUs: the profile check's read for `others_affinity`
/// ([`keep::others_problem`](super::super::keep::others_problem)). Opened
/// with the keeper's own rights, so a process it cannot hold is not
/// counted.
pub fn processes() -> Result<Processes, String> {
    let system = system_mask().ok_or("the PC's CPUs are unreadable")?;
    let mut now = ZERO;
    // SAFETY: one writable FILETIME.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    let now = ticks(&now);
    let mut list = Vec::new();
    for (pid, exe) in super::snapshot()? {
        let Ok(h) = Proc::open(pid) else { continue };
        if !h.image_is(&exe) {
            continue;
        }
        let (Some((affinity, _)), Some(created)) = (h.affinity(), h.created()) else {
            continue;
        };
        list.push(Seen {
            pid,
            exe,
            affinity,
            age_ms: age_ms(created, now),
        });
    }
    Ok(Processes { system, list })
}
