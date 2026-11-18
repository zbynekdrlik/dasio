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

/// Past this size the log moves to `.1` (the older `.1` goes).
pub const LOG_MAX_BYTES: u64 = 1 << 20;

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

/// How often a change to the same process is logged: a process something
/// else keeps setting back (ProBalance, a second keeper) logs one line a
/// minute with the count, not one every pass.
pub const QUIET_MS: u64 = 60_000;

/// Which changes to log, per process id.
#[derive(Debug, Default)]
pub struct Quiet {
    /// Per pid: when its last change was logged, and the changes since.
    seen: std::collections::HashMap<u32, (u64, u32)>,
}

impl Quiet {
    /// A change was made to `pid` at `now_ms`: `Some(n)` when it is to be
    /// logged (`n` the changes since the last logged one, this one
    /// included), `None` while it falls within [`QUIET_MS`] of that.
    pub fn note(&mut self, pid: u32, now_ms: u64) -> Option<u32> {
        match self.seen.get_mut(&pid) {
            Some((at, n)) if now_ms.saturating_sub(*at) < QUIET_MS => {
                *n += 1;
                None
            }
            Some((at, n)) => {
                let count = *n + 1;
                *at = now_ms;
                *n = 0;
                Some(count)
            }
            None => {
                self.seen.insert(pid, (now_ms, 0));
                Some(1)
            }
        }
    }

    /// Forgets processes that are gone (their ids come back for others).
    pub fn keep_only(&mut self, alive: &[u32]) {
        self.seen.retain(|pid, _| alive.contains(pid));
    }
}

/// The log line of a change, with how many changes it stands for.
pub fn counted(line: String, n: u32) -> String {
    if n > 1 {
        format!("{line} ({n} changes since the last line: something else sets it back)")
    } else {
        line
    }
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
