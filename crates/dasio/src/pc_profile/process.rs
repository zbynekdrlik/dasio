//! `[[process]]` entries: what a running process must have (affinity and
//! priority class, held by dasio's keeper since dasio#185, before that by
//! Process Lasso, and the token's integrity level, dasio#128, checked
//! only); skipped while the process does not run.

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
    #[serde(default)]
    pub why: String,
}

impl ProcEntry {
    /// Whether it checks anything at all.
    pub fn checks(&self) -> bool {
        self.affinity.is_some() || self.priority.is_some() || self.integrity.is_some()
    }
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
        let Some(s) = r.process(&e.exe) else { continue };
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
