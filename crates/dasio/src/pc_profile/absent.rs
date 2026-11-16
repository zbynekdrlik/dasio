//! `[[absent]]` entries (dasio#128, owner 16.11.2026): software a dasio PC
//! must not have at all, not only have switched off, so nothing starts "by
//! chance" and a Windows upgrade that brings it back is seen. The check
//! reports it while it exists (a WARN line and the tray's count, every
//! 10 min); `apply` removes exactly what the entry names:
//! - `exe = '<full path>'` with `uninstall = ['<uninstaller>', 'args'...]`:
//!   runs that uninstaller (its path may hold one `*` folder, e.g. Edge's
//!   version folder: the newest version found is run); when the exe is
//!   still there after it, an optional fallback stops the named processes
//!   running from the exe's own folder and deletes that folder, only one on
//!   [`REMOVABLE_DIRS`] (`remove_dir`, `stop_processes`: EdgeUpdate's
//!   `/uninstall` keeps itself while WebView2 is installed, dsg + mbc
//!   16.11.2026);
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

/// The folders an exe entry's fallback may delete (`remove_dir`; a delete
/// cannot be undone, so the list is in code, under review, never in a
/// profile): Edge's updater, whose `/uninstall` leaves itself while
/// WebView2 is installed (dasio#128, dsg + mbc 16.11.2026). WebView2's own
/// folder is not on it: it stays (owner 16.11.2026).
pub const REMOVABLE_DIRS: [&str; 1] = [r"C:\Program Files (x86)\Microsoft\EdgeUpdate"];

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
    /// The exe's own folder, deleted when the uninstaller left the exe
    /// (`exe` only, a folder on [`REMOVABLE_DIRS`]).
    #[serde(default)]
    pub remove_dir: Option<String>,
    /// Exe file names stopped before `remove_dir` is deleted, each only
    /// while it runs from inside that folder (`remove_dir` only).
    #[serde(default)]
    pub stop_processes: Option<Vec<String>>,
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

/// Whether `dir` is on [`REMOVABLE_DIRS`] (case ignored, as the file
/// system).
pub fn removable(dir: &str) -> bool {
    REMOVABLE_DIRS.iter().any(|d| d.eq_ignore_ascii_case(dir))
}

/// The folder a full path is in: `C:\a\b.exe` gives `C:\a`.
pub fn parent_dir(path: &str) -> Option<&str> {
    path.rsplit_once('\\').map(|(dir, _)| dir)
}

/// Whether the file `path` lies inside the folder `dir` (case ignored), at
/// any depth, with no `..` that could leave it.
pub fn inside(dir: &str, path: &str) -> bool {
    let head = path.get(..dir.len());
    let Some(rest) = head
        .filter(|h| h.eq_ignore_ascii_case(dir))
        .and_then(|_| path[dir.len()..].strip_prefix('\\'))
    else {
        return false;
    };
    !rest
        .split('\\')
        .any(|p| p.is_empty() || p == "." || p == "..")
}

/// A bare exe file name (`MicrosoftEdgeUpdate.exe`): no folder, no `*`.
fn exe_name(name: &str) -> bool {
    name.len() > 4
        && name.to_ascii_lowercase().ends_with(".exe")
        && !name.contains(['\\', '/', '*', ':'])
}

/// Why the fallback fields of exe entry `e` are wrong; `Ok` when they are
/// fine or absent.
fn validate_fallback(exe: &str, e: &AbsentEntry) -> Result<(), String> {
    if let Some(dir) = &e.remove_dir {
        if !removable(dir) {
            return Err(format!(
                "absent exe {exe}: dasio deletes only the folders {REMOVABLE_DIRS:?} (pc_profile::absent::REMOVABLE_DIRS), not {dir}"
            ));
        }
        if !parent_dir(exe).is_some_and(|p| p.eq_ignore_ascii_case(dir)) {
            return Err(format!(
                "absent exe {exe}: remove_dir {dir} must be the exe's own folder"
            ));
        }
    }
    if let Some(names) = &e.stop_processes {
        if e.remove_dir.is_none() {
            return Err(format!(
                "absent exe {exe}: stop_processes needs remove_dir (they stop only what runs from it)"
            ));
        }
        if let Some(bad) = names.iter().find(|n| !exe_name(n)) {
            return Err(format!(
                "absent exe {exe}: stop_processes {bad:?}: give an exe file name, e.g. MicrosoftEdgeUpdate.exe, no folder, no *"
            ));
        }
    }
    Ok(())
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
    if !matches!(target, Target::Exe(_)) && (e.remove_dir.is_some() || e.stop_processes.is_some()) {
        return Err(format!(
            "{}: remove_dir and stop_processes are for exe only",
            e.what()
        ));
    }
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
            validate_fallback(exe, e)?;
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
            // The folder is listed under System32\Tasks: no way out of it.
            let parts_ok = t
                .split('\\')
                .skip(1)
                .all(|p| !p.is_empty() && p != "." && p != "..");
            if !t.starts_with('\\') || leaf.is_empty() || leaf == "*" || !star_ok || !parts_ok {
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
    v.sort();
    v.into_iter().rev().map(|(_, n)| n).collect()
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

/// What `apply` does when an exe's uninstaller left the exe: stop the
/// named processes running from `dir`, then delete `dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    /// A folder on [`REMOVABLE_DIRS`], the exe's own.
    pub dir: String,
    /// Exe file names to stop while they run from inside `dir`.
    pub stop: Vec<String>,
}

/// One step of a [`Fallback`], in the order `apply` takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Stop every process of this exe name whose image is inside the
    /// folder (a running exe keeps its folder from being deleted).
    Stop { name: String, dir: String },
    /// Delete the folder and everything in it.
    RemoveDir(String),
}

impl Fallback {
    /// The steps: each process stopped first, then the folder deleted.
    pub fn steps(&self) -> Vec<Step> {
        self.stop
            .iter()
            .map(|name| Step::Stop {
                name: name.clone(),
                dir: self.dir.clone(),
            })
            .chain([Step::RemoveDir(self.dir.clone())])
            .collect()
    }
}

/// One removal `apply` makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// `sc stop` + `sc delete` of a service on [`DELETABLE_SERVICES`].
    Service(String),
    /// `schtasks /delete` of a task, by its full path as found.
    Task(String),
    /// The program's uninstaller, then a look that `exe` is gone; when it
    /// is not, the fallback (if the entry names one) and a second look.
    Exe {
        exe: String,
        uninstall: Vec<String>,
        fallback: Option<Fallback>,
    },
}

impl Removal {
    /// What `apply` does, for its log line.
    pub fn describe(&self) -> String {
        match self {
            Removal::Service(s) => format!("the absence of service {s} (sc stop + sc delete)"),
            Removal::Task(t) => format!("the absence of task {t} (schtasks /delete)"),
            Removal::Exe {
                exe,
                uninstall,
                fallback,
            } => {
                let then = fallback.as_ref().map_or(String::new(), |f| {
                    let stop = if f.stop.is_empty() {
                        String::new()
                    } else {
                        format!("stops {} inside it, ", f.stop.join(", "))
                    };
                    format!("; if that leaves the exe: {stop}deletes {}", f.dir)
                });
                format!("the absence of {exe} (runs {}{then})", uninstall.join(" "))
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
                    fallback: e.remove_dir.as_ref().map(|dir| Fallback {
                        dir: dir.clone(),
                        stop: e.stop_processes.clone().unwrap_or_default(),
                    }),
                },
                None => continue,
            });
        }
    }
    out
}
