//! `[[absent]]` entries (dasio#128, owner 16.11.2026): software a dasio PC
//! must not have at all, not only have switched off, so nothing starts "by
//! chance" and a Windows upgrade that brings it back is seen. The check
//! reports it while it exists (a WARN line and the tray's count, every
//! 10 min); `apply` removes exactly what the entry names:
//! - `exe = '<full path>'` with `uninstall = ['<uninstaller>', 'args'...]`:
//!   runs that uninstaller (its path may hold one `*` folder, e.g. Edge's
//!   version folder: the newest version found is run), after the registry
//!   values named in `before_uninstall` are written, each only one on
//!   [`PRE_UNINSTALL_WRITES`] (Edge's setup refuses on Windows 10 LTSC
//!   without EdgeUpdateDev `AllowUninstall`, FOH 17.11.2026); when the exe is
//!   still there after it, an optional fallback stops the named processes
//!   running from the exe's own folder and deletes that folder, only one on
//!   [`REMOVABLE_DIRS`] (`remove_dir`, `stop_processes`: EdgeUpdate's
//!   `/uninstall` keeps itself while WebView2 is installed, dsg + mbc
//!   16.11.2026); with `windows_caller = true` the uninstaller runs
//!   through a Windows caller (see [`caller_command`]);
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

/// A registry value `apply` writes right before an exe's uninstaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreWrite {
    /// The key under HKLM, without `HKLM\`.
    pub key: &'static str,
    /// The value's name.
    pub name: &'static str,
    /// Its REG_SZ data.
    pub data: &'static str,
}

impl PreWrite {
    /// The value as an entry's `before_uninstall` names it:
    /// `HKLM\<key>\<name>`.
    pub fn path(&self) -> String {
        format!(r"HKLM\{}\{}", self.key, self.name)
    }
}

/// The registry values an exe entry may have written before its
/// uninstaller (`before_uninstall`; the list and the data are in code,
/// under review, never in a profile). EdgeUpdateDev `AllowUninstall`
/// (REG_SZ ""): Edge's setup.exe refused `--uninstall --force-uninstall` on
/// FOH's Windows 10 IoT Enterprise LTSC 2021 ("Edge not uninstallable per
/// regional policy", EdgeUpdate's log, 17.11.2026; Microsoft supports the
/// uninstall only in the EEA on Windows 11), and the known removers
/// (he3als/EdgeRemover, ave9858's script) write exactly this value before
/// it (dasio#128). It stays after: it only lets setup uninstall Edge.
pub static PRE_UNINSTALL_WRITES: [PreWrite; 1] = [PreWrite {
    key: r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev",
    name: "AllowUninstall",
    data: "",
}];

/// The [`PRE_UNINSTALL_WRITES`] entry `path` names (`HKLM\<key>\<name>`,
/// case ignored, as the registry); `None` for any other.
pub fn pre_write(path: &str) -> Option<&'static PreWrite> {
    PRE_UNINSTALL_WRITES
        .iter()
        .find(|w| w.path().eq_ignore_ascii_case(path))
}

/// The program the Windows caller is copied from, under `%SystemRoot%`.
pub const CALLER_SOURCE: &str = r"System32\cmd.exe";

/// The Windows caller's file name: Edge's setup allows its uninstall when
/// its parent process is `dllhost.exe` in Windows' own folder (its log:
/// `Parent process: C:\Windows\SystemTemp\dllhost.exe`, `Uninstall
/// allowed: Windows caller`; FOH, Windows 10 IoT LTSC 2021, Edge 156,
/// 17.11.2026). A SYSTEM cmd.exe from System32, a setup.exe copied under
/// `C:\Windows\Temp` and an empty `windir` were each refused ("Not a
/// Windows caller" / "No windir environment variable found", exit 532).
/// The method is ave9858's Edge removal script (dasio#128).
pub const CALLER_NAME: &str = "dllhost.exe";

/// Where the Windows caller is copied to: `%SystemRoot%\SystemTemp` (the
/// SYSTEM account's temp folder, Windows 10 21H2+ and 11), else
/// `%SystemRoot%\Temp`. Deleted right after the uninstaller.
pub fn caller_path(system_root: &str, system_temp: bool) -> String {
    let dir = if system_temp { "SystemTemp" } else { "Temp" };
    format!(r"{system_root}\{dir}\{CALLER_NAME}")
}

/// Characters cmd.exe reads as its own inside a `/c` line (`"` would end
/// the quoting); an uninstaller or argument holding one is refused.
const CMD_SPECIAL: [char; 9] = ['"', '&', '|', '<', '>', '^', '%', '\n', '\r'];

/// Why `s` (an uninstaller path or argument run through the Windows
/// caller) cannot go on cmd.exe's `/c` line; `Ok` when it can.
fn cmd_safe(s: &str) -> Result<(), String> {
    match s.chars().find(|c| CMD_SPECIAL.contains(c)) {
        Some(c) => Err(format!(
            "{s:?} holds {c:?}, which cmd.exe reads itself (the Windows caller is cmd.exe)"
        )),
        None => Ok(()),
    }
}

/// The Windows caller's (cmd.exe's) command line that runs `prog args`:
/// `/c ""<prog>" <args>"`. cmd.exe strips the outer quotes of the `/c`
/// line, so a path with spaces or `(x86)` stays one quoted word.
pub fn caller_command(prog: &str, args: &[String]) -> Result<String, String> {
    cmd_safe(prog)?;
    for a in args {
        cmd_safe(a)?;
        if a.is_empty() || a.contains(char::is_whitespace) {
            return Err(format!(
                "{a:?}: an argument through the Windows caller is one word, no spaces"
            ));
        }
    }
    let mut line = format!(r#"/c ""{prog}""#);
    for a in args {
        line.push(' ');
        line.push_str(a);
    }
    line.push('"');
    Ok(line)
}

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
    /// Registry values written right before the uninstaller, each as
    /// `HKLM\<key>\<name>` and only one on [`PRE_UNINSTALL_WRITES`]
    /// (`exe` only).
    #[serde(default)]
    pub before_uninstall: Option<Vec<String>>,
    /// Run the uninstaller through a Windows caller (`exe` only): cmd.exe
    /// copied to [`caller_path`] as [`CALLER_NAME`], deleted after.
    #[serde(default)]
    pub windows_caller: bool,
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
    if !matches!(target, Target::Exe(_))
        && (e.remove_dir.is_some()
            || e.stop_processes.is_some()
            || e.before_uninstall.is_some()
            || e.windows_caller)
    {
        return Err(format!(
            "{}: remove_dir, stop_processes, before_uninstall and windows_caller are for exe only",
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
            if e.windows_caller {
                caller_command(prog, &cmd[1..])
                    .map_err(|err| format!("absent exe {exe}: windows_caller: {err}"))?;
            }
            if let Some(bad) = e
                .before_uninstall
                .iter()
                .flatten()
                .find(|p| pre_write(p).is_none())
            {
                let known: Vec<String> = PRE_UNINSTALL_WRITES.iter().map(PreWrite::path).collect();
                return Err(format!(
                    "absent exe {exe}: before_uninstall {bad}: dasio writes only {known:?} (pc_profile::absent::PRE_UNINSTALL_WRITES)"
                ));
            }
            let names = e.before_uninstall.as_deref().unwrap_or_default();
            if let Some((_, twice)) = names
                .iter()
                .enumerate()
                .find(|(i, p)| names[..*i].iter().any(|q| q.eq_ignore_ascii_case(p)))
            {
                return Err(format!(
                    "absent exe {exe}: before_uninstall {twice}: named twice"
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
    /// The registry values of `before` written, the program's uninstaller,
    /// then a look that `exe` is gone; when it is not, the fallback (if the
    /// entry names one) and a second look.
    Exe {
        exe: String,
        uninstall: Vec<String>,
        before: Vec<&'static PreWrite>,
        windows_caller: bool,
        fallback: Option<Fallback>,
    },
}

/// One step of an exe's removal, in the order `apply` takes them while the
/// exe is there (an exe already gone takes none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExeStep {
    /// Write this value (REG_SZ) under HKLM.
    Write(&'static PreWrite),
    /// Copy [`CALLER_SOURCE`] to [`caller_path`].
    CopyCaller,
    /// Run the uninstaller with its arguments (through the caller when
    /// one was copied).
    Uninstall(Vec<String>),
    /// Delete the caller's copy (also after a failed uninstaller).
    DeleteCaller,
    /// Look whether the exe is gone; when not, the fallback (if any).
    Look,
}

/// The steps of an exe removal: every `before` write first, then (with a
/// Windows caller: its copy, ) the uninstaller(, the copy's delete), then
/// the look.
pub fn exe_steps(
    uninstall: &[String],
    before: &[&'static PreWrite],
    windows_caller: bool,
) -> Vec<ExeStep> {
    let mut steps: Vec<ExeStep> = before.iter().copied().map(ExeStep::Write).collect();
    if windows_caller {
        steps.push(ExeStep::CopyCaller);
    }
    steps.push(ExeStep::Uninstall(uninstall.to_vec()));
    if windows_caller {
        steps.push(ExeStep::DeleteCaller);
    }
    steps.push(ExeStep::Look);
    steps
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
                before,
                windows_caller,
                fallback,
            } => {
                let via = if *windows_caller {
                    format!(" through {CALLER_NAME} (a copy of {CALLER_SOURCE})")
                } else {
                    String::new()
                };
                let first = if before.is_empty() {
                    String::new()
                } else {
                    let w: Vec<String> = before
                        .iter()
                        .map(|w| format!("{} = {:?}", w.path(), w.data))
                        .collect();
                    format!("sets {}, then ", w.join(", "))
                };
                let then = fallback.as_ref().map_or(String::new(), |f| {
                    let stop = if f.stop.is_empty() {
                        String::new()
                    } else {
                        format!("stops {} inside it, ", f.stop.join(", "))
                    };
                    format!("; if that leaves the exe: {stop}deletes {}", f.dir)
                });
                format!(
                    "the absence of {exe} ({first}runs {}{via}{then})",
                    uninstall.join(" ")
                )
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
                    // Validated at parse: every name is on the list.
                    before: e
                        .before_uninstall
                        .iter()
                        .flatten()
                        .filter_map(|p| pre_write(p.as_str()))
                        .collect(),
                    windows_caller: e.windows_caller,
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
