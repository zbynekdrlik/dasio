//! The process keeper (dasio#185, owner 18.11.2026: "ak vies vsetko co
//! robilo lasso riesit interne tak to rob interne"): what Process Lasso did
//! on a dasio PC, done by dasio. `dasio pc-profile keep` (a SYSTEM task,
//! `dasio-pc-keep`) walks the process list every [`EVERY_MS`] and gives
//! every running process of each `[[process]]` entry the entry's affinity
//! and priority class. The catch-all Lasso rule (everything else on the
//! system CPUs) is Windows' own `ReservedCpuSets` already.
//!
//! The decision is here; listing, reading and setting processes is
//! [`windows`](super::windows).

use super::{ProcEntry, ProcState, Profile, parse_mask};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    pub affinity: Option<u64>,
    /// One of [`super::PRIORITIES`].
    pub priority: Option<String>,
}

/// The entries the keeper holds: those naming an affinity or a priority
/// (`integrity` is only checked: a running process is never de-elevated).
pub fn held(p: &Profile) -> Vec<&ProcEntry> {
    p.process
        .iter()
        .filter(|e| e.affinity.is_some() || e.priority.is_some())
        .collect()
}

/// What `e` wants changed on a process in state `s`: each of affinity and
/// priority only when the process differs; `None` when nothing does.
pub fn wanted(e: &ProcEntry, s: &ProcState) -> Option<Set> {
    let affinity = e
        .affinity
        .as_deref()
        .and_then(parse_mask)
        .filter(|m| *m != s.affinity);
    let priority = e.priority.clone().filter(|p| *p != s.priority);
    if affinity.is_none() && priority.is_none() {
        return None;
    }
    Some(Set { affinity, priority })
}

/// The log line of one change made: what moved, from what.
pub fn line(exe: &str, pid: u32, s: &ProcState, set: &Set) -> String {
    let mut parts = Vec::new();
    if let Some(p) = &set.priority {
        parts.push(format!("priority {} -> {p}", s.priority));
    }
    if let Some(m) = set.affinity {
        parts.push(format!("affinity 0x{:X} -> 0x{m:X}", s.affinity));
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
