//! `[[absent]]` entries (dasio#128, owner 16.11.2026): software a dasio PC
//! must not have at all, not only have switched off, so nothing starts "by
//! chance" and a Windows upgrade that brings it back is seen. The check
//! reports it while it exists (a WARN line and the tray's count, every
//! 10 min); `apply` removes exactly what the entry names:
//! - `exe = '<full path>'` with `uninstall = ['<uninstaller>', 'args'...]`:
//!   runs that uninstaller (its path may hold one `*` folder, e.g. Edge's
//!   version folder: the newest version found is run);
//! - `service = '<name>'`: `sc stop` + `sc delete`, only for a service on
//!   [`DELETABLE_SERVICES`] (a parse error otherwise);
//! - `task = '<path as schtasks shows it>'`: `schtasks /delete`; a trailing
//!   `*` matches the rest of a name in that folder (Edge's updater adds a
//!   GUID to its task names), nothing else is a pattern.
//!
//! The portable part (validation, the check, the plan, the pattern and
//! version picks) is here; reading and removing is
//! [`windows`](super::windows).

use serde::Deserialize;

use super::{Problem, Reader};

/// The services `apply` may delete (`sc delete` cannot be undone, so the
/// list is in code, under review, never in a profile): Edge's updater and
/// its elevation service (dasio#128, dsg/FOH 16.11.2026).
pub const DELETABLE_SERVICES: [&str; 3] =
    ["edgeupdate", "edgeupdatem", "MicrosoftEdgeElevationService"];

/// Something that must not exist on the PC.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AbsentEntry {
    /// A program file, by its full path.
    #[serde(default)]
    pub exe: Option<String>,
    /// A service, by its name.
    #[serde(default)]
    pub service: Option<String>,
    /// A scheduled task, by its path (`\name`, `\folder\name`); a trailing
    /// `*` matches the rest of the name.
    #[serde(default)]
    pub task: Option<String>,
    /// The program's own uninstaller and its arguments (`exe` only).
    #[serde(default)]
    pub uninstall: Option<Vec<String>>,
    #[serde(default)]
    pub why: String,
}

/// What an entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    Exe(&'a str),
    Service(&'a str),
    Task(&'a str),
}

impl AbsentEntry {
    /// The one thing the entry names; `None` unless exactly one of `exe`,
    /// `service`, `task` is given.
    pub fn target(&self) -> Option<Target<'_>> {
        match (&self.exe, &self.service, &self.task) {
            (Some(e), None, None) => Some(Target::Exe(e)),
            (None, Some(s), None) => Some(Target::Service(s)),
            (None, None, Some(t)) => Some(Target::Task(t)),
            _ => None,
        }
    }

    /// The entry as a problem names it, e.g. `absent service edgeupdate`.
    pub fn what(&self) -> String {
        match self.target() {
            Some(Target::Exe(e)) => format!("absent exe {e}"),
            Some(Target::Service(s)) => format!("absent service {s}"),
            Some(Target::Task(t)) => format!("absent task {t}"),
            None => "absent (no target)".into(),
        }
    }

    /// Whether `other` names the same thing (a site entry replaces the
    /// baseline's).
    pub fn same_target(&self, other: &AbsentEntry) -> bool {
        match (self.target(), other.target()) {
            (Some(Target::Exe(a)), Some(Target::Exe(b)))
            | (Some(Target::Service(a)), Some(Target::Service(b)))
            | (Some(Target::Task(a)), Some(Target::Task(b))) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

/// Whether `service` is on [`DELETABLE_SERVICES`] (case ignored, as the SCM).
pub fn deletable(service: &str) -> bool {
    DELETABLE_SERVICES
        .iter()
        .any(|s| s.eq_ignore_ascii_case(service))
}

/// A full Windows path: a drive letter, a colon, a backslash.
fn full_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// Why `e` is not a valid entry; `Ok` when it is.
pub fn validate(e: &AbsentEntry) -> Result<(), String> {
    let Some(target) = e.target() else {
        return Err(format!(
            "absent {:?}: give exactly one of exe, service, task",
            [&e.exe, &e.service, &e.task]
        ));
    };
    match target {
        Target::Exe(exe) => {
            if !full_path(exe) || exe.contains('*') {
                return Err(format!(
                    "absent exe {exe}: give its full path, e.g. C:\\Program Files\\...\\x.exe, no *"
                ));
            }
            let Some(cmd) = e.uninstall.as_ref().filter(|c| !c.is_empty()) else {
                return Err(format!(
                    "absent exe {exe}: give uninstall, its uninstaller and arguments"
                ));
            };
            let prog = &cmd[0];
            if !full_path(prog) {
                return Err(format!(
                    "absent exe {exe}: the uninstaller {prog} needs its full path"
                ));
            }
            if prog.contains('*') && split_star(prog).is_none() {
                return Err(format!(
                    "absent exe {exe}: the uninstaller {prog} may hold one * as a whole folder (\\*\\)"
                ));
            }
        }
        Target::Service(s) => {
            if e.uninstall.is_some() {
                return Err(format!("absent service {s}: uninstall is for exe only"));
            }
            if !deletable(s) {
                return Err(format!(
                    "absent service {s}: dasio deletes only {DELETABLE_SERVICES:?} (pc_profile::absent::DELETABLE_SERVICES)"
                ));
            }
        }
        Target::Task(t) => {
            if e.uninstall.is_some() {
                return Err(format!("absent task {t}: uninstall is for exe only"));
            }
            let leaf = t.rsplit('\\').next().unwrap_or("");
            let star_ok = !t.strip_suffix('*').unwrap_or(t).contains('*');
            if !t.starts_with('\\') || leaf.is_empty() || leaf == "*" || !star_ok {
                return Err(format!(
                    "absent task {t}: give the path as schtasks shows it, e.g. \\MicrosoftEdgeUpdateTaskMachineCore*, a * only at the end after part of the name"
                ));
            }
        }
    }
    Ok(())
}

/// The folder `pattern` (a task path) lists in, without its leading `\`:
/// `""` for the root folder, `Microsoft\Windows` for
/// `\Microsoft\Windows\x`.
pub fn task_folder(pattern: &str) -> &str {
    let p = pattern.strip_prefix('\\').unwrap_or(pattern);
    p.rsplit_once('\\').map_or("", |(dir, _)| dir)
}

/// Whether the task `name` (its full path) is one `pattern` names: the
/// same path (case ignored), or for a trailing `*` the same folder and a
/// name starting with the rest.
pub fn task_matches(pattern: &str, name: &str) -> bool {
    let Some(prefix) = pattern.strip_suffix('*') else {
        return pattern.eq_ignore_ascii_case(name);
    };
    name.len() > prefix.len()
        && name.is_char_boundary(prefix.len())
        && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        && !name[prefix.len()..].contains('\\')
}

/// An uninstaller path with one `\*\` folder, split around it:
/// `C:\a\*\b\setup.exe` gives `(C:\a, b\setup.exe)`; `None` without exactly
/// one such `*`.
pub fn split_star(path: &str) -> Option<(&str, &str)> {
    if path.matches('*').count() != 1 {
        return None;
    }
    let (dir, rest) = path.split_once(r"\*\")?;
    (!dir.is_empty() && !rest.is_empty()).then_some((dir, rest))
}

/// The version folder names of `names` (dot-separated numbers only, e.g.
/// `156.0.4314.58`; `154.0.4258.62.deleting` is not one), newest first.
pub fn versions_newest_first(names: &[String]) -> Vec<&str> {
    let parse = |n: &str| -> Option<Vec<u64>> { n.split('.').map(|p| p.parse().ok()).collect() };
    let mut v: Vec<(Vec<u64>, &str)> = names
        .iter()
        .filter_map(|n| parse(n).map(|k| (k, n.as_str())))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, n)| n).collect()
}

/// The uninstaller to run for `prog`: itself without a `*`; else the
/// newest version folder (from `list`, the folder names under the part
/// before the `*`) whose uninstaller `exists`. `None` when none does.
pub fn resolve_uninstaller(
    prog: &str,
    list: impl Fn(&str) -> Vec<String>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let Some((dir, rest)) = split_star(prog) else {
        return (!prog.contains('*')).then(|| prog.to_string());
    };
    let names = list(dir);
    versions_newest_first(&names)
        .into_iter()
        .map(|v| format!(r"{dir}\{v}\{rest}"))
        .find(|p| exists(p))
}

/// One removal `apply` makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// `sc stop` + `sc delete` of a service on [`DELETABLE_SERVICES`].
    Service(String),
    /// `schtasks /delete` of a task, by its full path as found.
    Task(String),
    /// The program's uninstaller, then a look that `exe` is gone.
    Exe { exe: String, uninstall: Vec<String> },
}

impl Removal {
    /// What `apply` does, for its log line.
    pub fn describe(&self) -> String {
        match self {
            Removal::Service(s) => format!("the absence of service {s} (sc stop + sc delete)"),
            Removal::Task(t) => format!("the absence of task {t} (schtasks /delete)"),
            Removal::Exe { exe, uninstall } => {
                format!("the absence of {exe} (runs {})", uninstall.join(" "))
            }
        }
    }
}

/// Each entry of `entries` whose thing the PC has, as a problem.
pub fn check(entries: &[AbsentEntry], r: &dyn Reader) -> Vec<Problem> {
    entries
        .iter()
        .filter_map(|e| {
            let found = match r.absent(e) {
                Ok(found) if found.is_empty() => return None,
                Ok(found) => format!("present ({})", found.join(", ")),
                Err(err) => format!("unreadable ({err})"),
            };
            Some(Problem {
                what: e.what(),
                found,
                want: "absent".into(),
                why: e.why.clone(),
                must: false,
            })
        })
        .collect()
}

/// The removals that make the PC match `entries`: one per thing found.
/// Nothing is planned for an entry the PC could not be read for (the check
/// reports it).
pub fn plan(entries: &[AbsentEntry], r: &dyn Reader) -> Vec<Removal> {
    let mut out = Vec::new();
    for e in entries {
        let Ok(found) = r.absent(e) else { continue };
        for f in found {
            out.push(match e.target() {
                Some(Target::Service(s)) => Removal::Service(s.to_string()),
                Some(Target::Task(_)) => Removal::Task(f),
                Some(Target::Exe(exe)) => Removal::Exe {
                    exe: exe.to_string(),
                    uninstall: e.uninstall.clone().unwrap_or_default(),
                },
                None => continue,
            });
        }
    }
    out
}
