//! The process keeper (dasio#185, owner 18.11.2026: "ak vies vsetko co
//! robilo lasso riesit interne tak to rob interne"): what Process Lasso did
//! on a dasio PC, done by dasio. `dasio pc-profile keep` (a SYSTEM task,
//! `dasio-pc-keep`) walks the process list every [`EVERY_MS`] and gives
//! every running process of each `[[process]]` entry the entry's affinity,
//! priority class, I/O, memory and GPU priority and thread priority boost.
//! Lasso's catch-all (`*,0,0-1` on mbc: every other user process on the
//! system CPUs) is the profile's `others_affinity`: the keeper sets it on
//! every process no entry names ([`catch_all_takes`]) that still runs on
//! every CPU ([`others_wanted`]). Never `ReservedCpuSets` for that on a PC
//! with 2 system CPUs: it confines the kernel's threads too (mbc 19.11.2026,
//! its USB NIC's work on CPUs 0-1 only).
//!
//! The decision is here; listing, reading and setting processes is
//! [`windows`](super::windows).

use super::{Problem, ProcEntry, ProcState, Profile, Reader, parse_mask};

/// How often the keeper looks at the process list, in ms: a process
/// started since gets its entry within this.
pub const EVERY_MS: u64 = 2000;

/// The keeper's log: one line per change, size-bounded ([`LOG_MAX_BYTES`]).
pub const LOG_PATH: &str = r"C:\ProgramData\dasio\pc-keep.log";

/// While this file exists the keeper ends at its next pass (within
/// [`EVERY_MS`]): install.ps1 and uninstall.ps1 stop it this way before
/// they replace or remove its exe (a scheduled task's stop left it running
/// in CI), and remove the file before they start it again.
pub const STOP_PATH: &str = r"C:\ProgramData\dasio\pc-keep.stop";

/// Past this size the log moves to `.1` (the older `.1` goes).
pub const LOG_MAX_BYTES: u64 = 1_048_576;

/// What the keeper sets on one process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Set {
    pub affinity: Option<u64>,
    /// One of [`super::PRIORITIES`].
    pub priority: Option<String>,
    /// One of [`super::IO_PRIORITIES`].
    pub io: Option<String>,
    /// One of [`super::MEMORY_PRIORITIES`].
    pub memory: Option<String>,
    /// One of [`super::GPU_PRIORITIES`].
    pub gpu: Option<String>,
    /// The thread priority boost on or off.
    pub boost: Option<bool>,
}

impl Set {
    /// Whether it sets nothing.
    pub fn is_empty(&self) -> bool {
        *self == Set::default()
    }
}

/// The entries the keeper holds: those naming anything it sets
/// (`integrity` is only checked: a running process is never de-elevated).
pub fn held(p: &Profile) -> Vec<&ProcEntry> {
    p.process.iter().filter(|e| e.holds()).collect()
}

/// What `e` wants changed on a process in state `s`: each field only when
/// the process differs; one read as unreadable (`None` in [`super::Extras`])
/// is left alone (the profile check names it). `None` when nothing differs.
pub fn wanted(e: &ProcEntry, s: &ProcState) -> Option<Set> {
    fn differs<T: PartialEq + Clone>(want: &Option<T>, found: &Option<T>) -> Option<T> {
        match (want, found) {
            (Some(w), Some(f)) if w != f => Some(w.clone()),
            _ => None,
        }
    }
    let set = Set {
        affinity: e
            .affinity
            .as_deref()
            .and_then(parse_mask)
            .filter(|m| *m != s.affinity),
        priority: e.priority.clone().filter(|p| *p != s.priority),
        io: differs(&e.io_priority, &s.extra.io),
        memory: differs(&e.memory_priority, &s.extra.memory),
        gpu: differs(&e.gpu_priority, &s.extra.gpu),
        boost: differs(&e.priority_boost, &s.extra.boost),
    };
    (!set.is_empty()).then_some(set)
}

/// The log line of one change made: what moved, from what.
pub fn line(exe: &str, pid: u32, s: &ProcState, set: &Set) -> String {
    let was = |v: &Option<String>| v.clone().unwrap_or_else(|| "?".into());
    let mut parts = Vec::new();
    if let Some(p) = &set.priority {
        parts.push(format!("priority {} -> {p}", s.priority));
    }
    if let Some(m) = set.affinity {
        parts.push(format!("affinity 0x{:X} -> 0x{m:X}", s.affinity));
    }
    if let Some(v) = &set.io {
        parts.push(format!("io priority {} -> {v}", was(&s.extra.io)));
    }
    if let Some(v) = &set.memory {
        parts.push(format!("memory priority {} -> {v}", was(&s.extra.memory)));
    }
    if let Some(v) = &set.gpu {
        parts.push(format!("gpu priority {} -> {v}", was(&s.extra.gpu)));
    }
    if let Some(v) = set.boost {
        parts.push(format!(
            "priority boost {} -> {}",
            was(&s.extra.boost.map(super::boost_name)),
            super::boost_name(v)
        ));
    }
    format!("kept {exe} (pid {pid}): {}", parts.join(", "))
}

/// A process set back this many times within [`FIGHT_WINDOW_MS`] is given
/// up: something else sets it (ProBalance, a second rule), and an endless
/// fight helps nobody. The 10-min profile check shows what is left.
pub const FIGHT_LIMIT: u32 = 5;

/// The window [`FIGHT_LIMIT`] counts in, in ms.
pub const FIGHT_WINDOW_MS: u64 = 600_000;

/// What a change to a process means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// Logged and kept on holding it.
    Changed,
    /// The [`FIGHT_LIMIT`]-th change in its window: logged, then given up.
    GiveUp,
}

/// The changes made per process id, to give up a fight.
#[derive(Debug, Default)]
pub struct Fights {
    /// Per pid: when its window began, and the changes in it.
    seen: std::collections::HashMap<u32, (u64, u32)>,
    gave_up: std::collections::HashSet<u32>,
}

impl Fights {
    /// Whether the keeper gave `pid` up.
    pub fn gave_up(&self, pid: u32) -> bool {
        self.gave_up.contains(&pid)
    }

    /// A change was made to `pid` at `now_ms`.
    pub fn note(&mut self, pid: u32, now_ms: u64) -> Note {
        let (start, n) = self.seen.entry(pid).or_insert((now_ms, 0));
        if now_ms.saturating_sub(*start) >= FIGHT_WINDOW_MS {
            *start = now_ms;
            *n = 0;
        }
        *n += 1;
        if *n >= FIGHT_LIMIT {
            self.gave_up.insert(pid);
            Note::GiveUp
        } else {
            Note::Changed
        }
    }

    /// Forgets processes that are gone (their ids come back for others).
    pub fn keep_only(&mut self, alive: &[u32]) {
        self.seen.retain(|pid, _| alive.contains(pid));
        self.gave_up.retain(|pid| alive.contains(pid));
    }
}

/// The log line of a process given up.
pub fn gave_up_line(exe: &str, pid: u32) -> String {
    format!(
        "gave up keeping {exe} (pid {pid}): something else set it back {FIGHT_LIMIT} times in {} min; the PC profile check shows what differs",
        FIGHT_WINDOW_MS / 60_000
    )
}

/// Why `mask` cannot be set on this PC, whose CPUs are `system`: a CPU it
/// does not have.
pub fn mask_outside(mask: u64, system: u64) -> Option<String> {
    (mask & !system != 0).then(|| {
        format!("affinity 0x{mask:X} names CPUs this PC does not have (it has 0x{system:X})")
    })
}

/// `unix_s` as `YYYY-MM-DDTHH:MM:SSZ` (UTC), the keeper log's stamp.
pub fn utc_stamp(unix_s: u64) -> String {
    let s = unix_s % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate::minutes::date(unix_s),
        s / 3_600,
        s % 3_600 / 60,
        s % 60
    )
}

/// The session-0 core the catch-all never takes, by image name (dasio#185):
/// the session manager, the Win32 subsystem, the boot-time init, the
/// service control manager, the local security authority and the logon
/// process. Four are protected processes the keeper cannot open anyway;
/// lsass and winlogon serve every logon, authentication and desktop switch,
/// and confining them gains the DAW nothing. Their children are taken like
/// any other process (a service started by services.exe begins on every
/// CPU). Widened only by a reviewed code change, never from a profile.
pub const NEVER: [&str; 6] = [
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "winlogon.exe",
];

/// Whether `exe` is one of dasio's own: `dasio.exe` (the check's child,
/// install.ps1's runs) or `dasio-*` (each instance, `dasio-keep.exe`).
pub fn dasio_exe(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    exe == "dasio.exe" || exe.starts_with("dasio-")
}

/// Whether the catch-all (`others_affinity`) takes the process `pid`
/// named `exe` (its file name, as the process list gives it); `own` is the
/// caller's process id. Never one any `[[process]]` entry names (a
/// check-only one too: the baseline names both Lives for `integrity`, so a
/// DAW never lands on the system CPUs), the caller itself, a dasio exe, the
/// [`NEVER`] list, or a name without `.exe`: the kernel's pseudo-processes
/// (`[System Process]` (idle), `System`, `Registry`, `Memory Compression`,
/// `Secure System`, `vmmem`) have no image file.
pub fn catch_all_takes(p: &Profile, exe: &str, pid: u32, own: u32) -> bool {
    pid != own
        && exe.to_ascii_lowercase().ends_with(".exe")
        && !dasio_exe(exe)
        && !NEVER.iter().any(|n| n.eq_ignore_ascii_case(exe))
        && !p.process.iter().any(|e| e.exe.eq_ignore_ascii_case(exe))
}

/// The mask `others_affinity` asks for on this PC, whose CPUs are
/// `system`: `None` without the key; why not, when it names a CPU the PC
/// lacks or was not resolved ([`super::resolve`] puts a placeholder's mask
/// in).
pub fn others(p: &Profile, system: u64) -> Result<Option<u64>, String> {
    let Some(text) = &p.others_affinity else {
        return Ok(None);
    };
    let mask = parse_mask(text)
        .ok_or_else(|| format!("others_affinity {text} is not a mask (not resolved)"))?;
    match mask_outside(mask, system) {
        Some(why) => Err(format!("others_{why}")),
        None => Ok(Some(mask)),
    }
}

/// What the catch-all sets on a process at `found` on a PC whose CPUs are
/// `system`: `want`, only while the process runs on every CPU and that
/// differs. A process on any other mask set its own (Lasso's
/// DoNotAdjustAffinityIfCustomized), or the keeper set it already: left
/// alone. So a changed `others_affinity` reaches a process at its next
/// start.
pub fn others_wanted(want: u64, found: u64, system: u64) -> Option<u64> {
    (found == system && found != want).then_some(want)
}

/// The log line of a process the catch-all set.
pub fn others_line(exe: &str, pid: u32, from: u64, to: u64) -> String {
    format!("kept {exe} (pid {pid}): affinity 0x{from:X} -> 0x{to:X} (others_affinity)")
}

/// The keeper's line at a profile load about its catch-all, on a PC whose
/// CPUs are `system`; `None` without `others_affinity`.
pub fn others_loaded(p: &Profile, system: u64) -> Option<String> {
    Some(match others(p, system).transpose()? {
        Ok(mask) => format!(
            "every process no [[process]] entry names goes on 0x{mask:X} (others_affinity), unless it set its own affinity"
        ),
        Err(why) => format!("the catch-all holds nothing: {why}"),
    })
}

/// A running process as the catch-all's check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub pid: u32,
    /// Its exe file name, as the process list gives it.
    pub exe: String,
    pub affinity: u64,
    /// Since it started, in ms.
    pub age_ms: u64,
}

/// Every process the keeper could open to set, with the PC's CPUs
/// ([`Reader::processes`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Processes {
    /// Every CPU of the PC (the system affinity mask).
    pub system: u64,
    pub list: Vec<Seen>,
}

/// A process younger than this is not counted as not held: the keeper has
/// not had its passes on it yet (5 passes).
pub const GRACE_MS: u64 = 5 * EVERY_MS;

/// How many exe names the catch-all's problem shows.
pub const SHOWN: usize = 5;

/// Milliseconds between two FILETIMEs (100 ns units): a process's age.
pub fn age_ms(created: u64, now: u64) -> u64 {
    now.saturating_sub(created) / 10_000
}

/// The check's one problem for `others_affinity` (dasio#185), never one
/// per process: the processes the catch-all takes that still run on every
/// CPU, older than [`GRACE_MS`], by distinct exe name in name order (at most
/// [`SHOWN`] named), so its text stays the same while the same exes stay
/// unheld (the service logs a problem again only when its text changes).
/// `own`: the checking process. `None` without the key (nothing is read)
/// or when every such process is held.
pub fn others_problem(p: &Profile, r: &dyn Reader, own: u32) -> Option<Problem> {
    let text = p.others_affinity.as_ref()?;
    let problem = |found: String, want: String| {
        Problem {
        what: "others_affinity".into(),
        found,
        want,
        why: "every process no [[process]] entry names, held by dasio's keeper as Process Lasso's * rule did (dasio#185)".into(),
        must: false,
    }
    };
    let procs = match r.processes() {
        Ok(procs) => procs,
        Err(e) => return Some(problem(format!("unreadable ({e})"), text.clone())),
    };
    let want = match others(p, procs.system) {
        Ok(want) => want?,
        Err(why) => return Some(problem(why, "CPUs this PC has".into())),
    };
    let mut names: Vec<&str> = procs
        .list
        .iter()
        .filter(|s| {
            s.age_ms >= GRACE_MS
                && catch_all_takes(p, &s.exe, s.pid, own)
                && others_wanted(want, s.affinity, procs.system).is_some()
        })
        .map(|s| s.exe.as_str())
        .collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    if names.is_empty() {
        return None;
    }
    let shown = names[..names.len().min(SHOWN)].join(", ");
    let more = match names.len().saturating_sub(SHOWN) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    Some(problem(
        format!(
            "not held: {shown}{more} on every CPU (0x{:X})",
            procs.system
        ),
        format!("0x{want:X}"),
    ))
}
