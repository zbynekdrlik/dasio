//! `[[process]]` entries: what a running process must have (affinity,
//! priority class, I/O, memory and GPU priority and its thread priority
//! boost, held by dasio's keeper since dasio#185, before that by Process
//! Lasso, and the token's integrity level, dasio#128, checked only);
//! skipped while the process does not run.

use serde::Deserialize;

use super::{Problem, Profile, Reader, parse_mask};

/// What a running process must have.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcEntry {
    /// The image name, e.g. `AbletonLiveMaster.exe` (case ignored).
    pub exe: String,
    /// The affinity mask in hex, e.g. `"0xF0"`.
    #[serde(default)]
    pub affinity: Option<String>,
    /// `idle`, `below normal`, `normal`, `above normal`, `high`, `realtime`.
    #[serde(default)]
    pub priority: Option<String>,
    /// The token's integrity level, one of [`INTEGRITIES`] (dasio#128,
    /// 17.11.2026: a Live started from the elevated remote shell runs at
    /// `high`, and fohmixer's hub, at `medium`, may not move its windows).
    /// Checked only: a running process cannot be de-elevated.
    #[serde(default)]
    pub integrity: Option<String>,
    /// One of [`IO_PRIORITIES`] (Lasso's `DefaultIOPriorities`, dasio#185).
    #[serde(default)]
    pub io_priority: Option<String>,
    /// One of [`MEMORY_PRIORITIES`] (Lasso's `DefaultMemoryPriorities`).
    #[serde(default)]
    pub memory_priority: Option<String>,
    /// One of [`GPU_PRIORITIES`] (Lasso's `DefaultGPUPriorities`).
    #[serde(default)]
    pub gpu_priority: Option<String>,
    /// Windows' dynamic priority boost of its threads: `false` turns it off
    /// (Lasso's `ThreadPriorityBoosts=<exe>;0`).
    #[serde(default)]
    pub priority_boost: Option<bool>,
    #[serde(default)]
    pub why: String,
}

impl ProcEntry {
    /// Whether it checks anything at all.
    pub fn checks(&self) -> bool {
        self.holds() || self.integrity.is_some()
    }

    /// The rights a process handle needs, beyond a limited query, to read
    /// the [`Extras`] it names.
    pub fn extra_access(&self) -> ExtraAccess {
        if self.gpu_priority.is_some() {
            ExtraAccess::QueryAndSet
        } else if self.io_priority.is_some()
            || self.memory_priority.is_some()
            || self.priority_boost.is_some()
        {
            ExtraAccess::Query
        } else {
            ExtraAccess::None
        }
    }

    /// Whether it names anything the keeper sets (all but `integrity`).
    pub fn holds(&self) -> bool {
        self.affinity.is_some()
            || self.priority.is_some()
            || self.io_priority.is_some()
            || self.memory_priority.is_some()
            || self.gpu_priority.is_some()
            || self.priority_boost.is_some()
    }
}

/// I/O priorities by value (ntdll's ProcessIoPriority, 0-3; 4, critical,
/// is the memory manager's, never wanted).
pub const IO_PRIORITIES: [&str; 4] = ["very low", "low", "normal", "high"];

/// I/O priorities as read: [`IO_PRIORITIES`] and 4, critical, so a process
/// at critical reads as that, never as unreadable.
pub const IO_READ: [&str; 5] = ["very low", "low", "normal", "high", "critical"];

/// Memory priorities, value 1 first (`MEMORY_PRIORITY_VERY_LOW` 1 to
/// `MEMORY_PRIORITY_NORMAL` 5).
pub const MEMORY_PRIORITIES: [&str; 5] = ["very low", "low", "medium", "below normal", "normal"];

/// GPU scheduling priority classes by value (`D3DKMT_SCHEDULINGPRIORITYCLASS`
/// 0-5).
pub const GPU_PRIORITIES: [&str; 6] = [
    "idle",
    "below normal",
    "normal",
    "above normal",
    "high",
    "realtime",
];

/// The name of `value` in `names` (`first` is the value of `names[0]`).
pub fn level_name(names: &[&'static str], first: u32, value: u32) -> Option<&'static str> {
    names.get(value.checked_sub(first)? as usize).copied()
}

/// The value of `name` in `names` (`first` is the value of `names[0]`).
pub fn level_value(names: &[&str], first: u32, name: &str) -> Option<u32> {
    names
        .iter()
        .position(|n| *n == name)
        .map(|i| first + i as u32)
}

/// The rights a handle needs to read an entry's [`Extras`], beyond a
/// limited query: a full query for I/O or memory priority or the boost; for
/// the GPU class the set right too (gdi32's get refuses a handle without
/// it: STATUS_ACCESS_DENIED with the query right alone, the class read with
/// both, dsg 18.11.2026). Read through a handle of their own, so a process
/// that refuses them still has its affinity and priority kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraAccess {
    None,
    Query,
    QueryAndSet,
}

/// What a running process has beyond affinity and priority class, read
/// only when its entry names it; `None`: not read, or unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras {
    pub io: Option<String>,
    pub memory: Option<String>,
    pub gpu: Option<String>,
    pub boost: Option<bool>,
}

/// A running process as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcState {
    pub affinity: u64,
    /// One of the names [`ProcEntry::priority`] takes.
    pub priority: String,
    /// The token's integrity level ([`integrity_name`]), or why it could
    /// not be read.
    pub integrity: Result<String, String>,
    /// The rest the entry names ([`Extras`]).
    pub extra: Extras,
}

/// The integrity levels a `[[process]]` entry may want.
pub const INTEGRITIES: [&str; 4] = ["low", "medium", "high", "system"];

/// The name of a token's integrity level from its mandatory label's RID
/// (`SECURITY_MANDATORY_*_RID`): `untrusted`, `medium plus` (UIAccess)
/// and `protected` are read but never wanted.
pub fn integrity_name(rid: u32) -> &'static str {
    match rid {
        0..0x1000 => "untrusted",
        0x1000..0x2000 => "low",
        0x2000..0x2100 => "medium",
        0x2100..0x3000 => "medium plus",
        0x3000..0x4000 => "high",
        0x4000..0x5000 => "system",
        _ => "protected",
    }
}

/// The problems of `p`'s `[[process]]` entries, added to `out`.
pub(super) fn problems(p: &Profile, r: &dyn Reader, out: &mut Vec<Problem>) {
    for e in &p.process {
        let Some(s) = r.process(e) else { continue };
        if let Some(want) = e.affinity.as_deref().and_then(parse_mask)
            && s.affinity != want
        {
            out.push(Problem {
                what: format!("process {} affinity", e.exe),
                found: format!("0x{:X}", s.affinity),
                want: format!("0x{want:X}"),
                why: e.why.clone(),
                must: false,
            });
        }
        if let Some(want) = &e.priority
            && s.priority != *want
        {
            out.push(Problem {
                what: format!("process {} priority", e.exe),
                found: s.priority.clone(),
                want: want.clone(),
                why: e.why.clone(),
                must: false,
            });
        }
        let mut level = |what: &str, want: Option<String>, found: Option<String>| {
            if let Some(want) = want
                && found.as_ref() != Some(&want)
            {
                out.push(Problem {
                    what: format!("process {} {what}", e.exe),
                    found: found.unwrap_or_else(|| "unreadable".into()),
                    want,
                    why: e.why.clone(),
                    must: false,
                });
            }
        };
        level("io priority", e.io_priority.clone(), s.extra.io.clone());
        level(
            "memory priority",
            e.memory_priority.clone(),
            s.extra.memory.clone(),
        );
        level("gpu priority", e.gpu_priority.clone(), s.extra.gpu.clone());
        level(
            "priority boost",
            e.priority_boost.map(boost_name),
            s.extra.boost.map(boost_name),
        );
        if let Some(want) = &e.integrity {
            let found = match &s.integrity {
                Ok(level) if level == want => None,
                Ok(level) => Some(level.clone()),
                Err(err) => Some(format!("unreadable ({err})")),
            };
            if let Some(found) = found {
                out.push(Problem {
                    what: format!("process {} integrity", e.exe),
                    found,
                    want: want.clone(),
                    why: e.why.clone(),
                    must: false,
                });
            }
        }
    }
}

/// How a priority boost setting reads in a problem and the keeper's log.
pub fn boost_name(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}
