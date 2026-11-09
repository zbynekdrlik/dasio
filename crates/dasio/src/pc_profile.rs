//! The PC profile (dasio#5, owner 4.11.2026): the Windows settings a PC
//! must keep for dasio, declared once in a private TOML file on the PC
//! (`C:\ProgramData\dasio\pc-profile.toml`, site data, never in the repo),
//! so a Windows upgrade or a reinstall never means finding them again.
//! The service checks it at start and every [`CHECK_EVERY_S`], in a child
//! process off the media CPUs ([`ChildCheck`]), and reports what differs;
//! `dasio pc-profile apply` sets what differs.
//!
//! Four kinds of entries:
//! - `[[registry]]`: one value under `HKLM\...` (a DWORD, a string, or
//!   binary as hex). `{nic_class}` and `{nic_device}` in a key stand for
//!   the Dante NIC's class key and its Enum `Device Parameters` key, found
//!   from `nic_guid`.
//! - `[[ini]]`: one `key=value` line of an ini file (Process Lasso's).
//! - `[[process]]`: what a running process must have (affinity, priority
//!   class), to see that the tool which sets it (Lasso) did; checked only,
//!   never applied, and skipped while the process does not run.
//! - `[[disk]]`: a floor of free space on a drive, so a filling disk shows
//!   as a problem before it hurts (FOH's C: had 26.5 GB left, owner
//!   9.11.2026, dasio#1); checked only, never applied.
//!
//! This part is portable and tested everywhere; reading and writing the
//! PC is [`windows`](self::windows).

use serde::Deserialize;

#[cfg(windows)]
pub mod windows;

/// Where the service looks for the profile.
pub const DEFAULT_PATH: &str = r"C:\ProgramData\dasio\pc-profile.toml";
/// Seconds between the service's checks.
pub const CHECK_EVERY_S: u64 = 600;

/// One PC's profile.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// The PC it belongs to (shown in the log and the tray).
    pub name: String,
    /// The Dante NIC's GUID (NetCfgInstanceId), for `{nic_class}` and
    /// `{nic_device}`.
    #[serde(default)]
    pub nic_guid: Option<String>,
    #[serde(default)]
    pub registry: Vec<RegEntry>,
    #[serde(default)]
    pub ini: Vec<IniEntry>,
    #[serde(default)]
    pub process: Vec<ProcEntry>,
    #[serde(default)]
    pub disk: Vec<DiskEntry>,
}

/// An expected registry value: exactly one of `dword`, `string`, `binary`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegEntry {
    /// `HKLM\...`, possibly with `{nic_class}` / `{nic_device}`.
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub dword: Option<u32>,
    #[serde(default)]
    pub string: Option<String>,
    /// Hex bytes, e.g. `"0f00000000000000"`.
    #[serde(default)]
    pub binary: Option<String>,
    /// Why (the evidence): shown with a mismatch.
    #[serde(default)]
    pub why: String,
}

/// An expected `key=value` line in an ini file.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IniEntry {
    pub file: String,
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub why: String,
}

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
    #[serde(default)]
    pub why: String,
}

/// A floor of free space on a drive.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiskEntry {
    /// The drive, e.g. `C:`.
    pub drive: String,
    /// The least free space it may have, in GB (2^30 bytes, as Explorer
    /// shows it).
    pub min_free_gb: u64,
    #[serde(default)]
    pub why: String,
}

/// Bytes in the GB the profile and its problems use.
const GB: u64 = 1 << 30;

/// A registry value as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Dword(u32),
    String(String),
    Binary(Vec<u8>),
}

/// A running process as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcState {
    pub affinity: u64,
    /// One of the names [`ProcEntry::priority`] takes.
    pub priority: String,
}

/// What the PC shows, for the check.
pub trait Reader {
    /// The value `name` under `key`; `Ok(None)` when it (or the key) is
    /// absent.
    fn reg(&self, key: &str, name: &str) -> Result<Option<Value>, String>;
    /// The value of the first `key=` line of `file`; `Ok(None)` without one.
    fn ini(&self, file: &str, key: &str) -> Result<Option<String>, String>;
    /// The first running process with image `exe`; `None` when none runs.
    fn process(&self, exe: &str) -> Option<ProcState>;
    /// The free bytes on `drive` (e.g. `C:`) for the reading account.
    fn free_bytes(&self, drive: &str) -> Result<u64, String>;
}

/// One setting that is not as the profile says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub what: String,
    pub found: String,
    pub want: String,
    pub why: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} (the profile wants {})",
            self.what, self.found, self.want
        )?;
        if !self.why.is_empty() {
            write!(f, " — {}", self.why)?;
        }
        Ok(())
    }
}

/// Reads a profile; the error names what is wrong in it.
pub fn parse(text: &str) -> Result<Profile, String> {
    let p: Profile = toml::from_str(text).map_err(|e| e.to_string())?;
    for r in &p.registry {
        if want_reg(r).is_none() {
            return Err(format!(
                "registry {}\\{}: give exactly one of dword, string, binary (binary as hex)",
                r.key, r.name
            ));
        }
        if (r.key.contains("{nic_class}") || r.key.contains("{nic_device}")) && p.nic_guid.is_none()
        {
            return Err(format!("registry {}: needs nic_guid", r.key));
        }
    }
    for e in &p.process {
        if let Some(a) = &e.affinity
            && parse_mask(a).is_none()
        {
            return Err(format!("process {}: affinity {a} is not a hex mask", e.exe));
        }
        if let Some(c) = &e.priority
            && !PRIORITIES.contains(&c.as_str())
        {
            return Err(format!(
                "process {}: priority {c} is not one of {PRIORITIES:?}",
                e.exe
            ));
        }
    }
    for d in &p.disk {
        let b = d.drive.as_bytes();
        if b.len() != 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
            return Err(format!(
                "disk {}: give the drive as a letter and a colon, e.g. C:",
                d.drive
            ));
        }
        if d.min_free_gb == 0 {
            return Err(format!("disk {}: min_free_gb must be above 0", d.drive));
        }
    }
    Ok(p)
}

/// The priority class names, as [`ProcState::priority`] gives them.
pub const PRIORITIES: [&str; 6] = [
    "idle",
    "below normal",
    "normal",
    "above normal",
    "high",
    "realtime",
];

/// The value a registry entry wants; `None` unless exactly one of
/// `dword`, `string`, `binary` (valid hex) is given.
pub fn want_reg(r: &RegEntry) -> Option<Value> {
    match (&r.dword, &r.string, &r.binary) {
        (Some(d), None, None) => Some(Value::Dword(*d)),
        (None, Some(s), None) => Some(Value::String(s.clone())),
        (None, None, Some(b)) => hex(b).map(Value::Binary),
        _ => None,
    }
}

fn hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A hex mask, `0x` optional.
pub fn parse_mask(s: &str) -> Option<u64> {
    let s = s.trim();
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u64::from_str_radix(s, 16).ok()
}

fn show(v: &Value) -> String {
    match v {
        Value::Dword(d) => format!("{d} (0x{d:X})"),
        Value::String(s) => format!("\"{s}\""),
        Value::Binary(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
    }
}

/// Every setting of `p` the PC does not have.
pub fn check(p: &Profile, r: &dyn Reader) -> Vec<Problem> {
    let mut out = Vec::new();
    for e in &p.registry {
        let Some(want) = want_reg(e) else { continue };
        let found = r.reg(&e.key, &e.name);
        let found_text = match &found {
            Ok(Some(v)) if *v == want => continue,
            Ok(Some(v)) => show(v),
            Ok(None) => "absent".to_string(),
            Err(err) => format!("unreadable ({err})"),
        };
        out.push(Problem {
            what: format!("registry {}\\{}", e.key, e.name),
            found: found_text,
            want: show(&want),
            why: e.why.clone(),
        });
    }
    for e in &p.ini {
        let found = match r.ini(&e.file, &e.key) {
            Ok(Some(v)) if v == e.value => continue,
            Ok(Some(v)) => format!("\"{v}\""),
            Ok(None) => "absent".to_string(),
            Err(err) => format!("unreadable ({err})"),
        };
        out.push(Problem {
            what: format!("{} {}", e.file, e.key),
            found,
            want: format!("\"{}\"", e.value),
            why: e.why.clone(),
        });
    }
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
            });
        }
    }
    for e in &p.disk {
        // The text stays the same while the drive stays under its floor:
        // the service logs a problem again only when its text changes, so
        // the live amount (which moves all the time) is not in it. The
        // check command prints the amount on its own line ([`gb_down`]).
        let found = match r.free_bytes(&e.drive) {
            Ok(free) if free >= e.min_free_gb.saturating_mul(GB) => continue,
            Ok(_) => "below the floor".to_string(),
            Err(err) => format!("unreadable ({err})"),
        };
        out.push(Problem {
            what: format!("disk {} free space", e.drive),
            found,
            want: format!("at least {} GB", e.min_free_gb),
            why: e.why.clone(),
        });
    }
    out
}

/// `bytes` in GB with one decimal, rounded down, so an amount under a
/// floor never reads as the floor itself.
pub fn gb_down(bytes: u64) -> String {
    let tenths = u128::from(bytes) * 10 / u128::from(GB);
    format!("{}.{} GB", tenths / 10, tenths % 10)
}

/// One change `apply` makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Reg {
        key: String,
        name: String,
        value: Value,
    },
    Ini {
        file: String,
        key: String,
        value: String,
    },
}

/// What has to change for the PC to match (processes are only checked:
/// the tool that sets them, Lasso, is configured through its ini). An
/// unreadable value is planned too: the write says why it fails.
pub fn plan(p: &Profile, r: &dyn Reader) -> Vec<Action> {
    let mut out = Vec::new();
    for e in &p.registry {
        let Some(want) = want_reg(e) else { continue };
        if !matches!(r.reg(&e.key, &e.name), Ok(Some(ref v)) if *v == want) {
            out.push(Action::Reg {
                key: e.key.clone(),
                name: e.name.clone(),
                value: want,
            });
        }
    }
    for e in &p.ini {
        if !matches!(r.ini(&e.file, &e.key), Ok(Some(ref v)) if *v == e.value) {
            out.push(Action::Ini {
                file: e.file.clone(),
                key: e.key.clone(),
                value: e.value.clone(),
            });
        }
    }
    out
}

/// What an applied action needs before it takes effect.
pub fn takes_effect(a: &Action) -> &'static str {
    match a {
        Action::Reg { key, .. } if key.starts_with("{nic_") => "a restart of the Dante NIC",
        Action::Reg { key, name, .. }
            if name == "Start" && key.to_ascii_lowercase().contains(r"\services\") =>
        {
            "a reboot"
        }
        // Environment is read at logon (services read it at boot); every
        // other Session Manager key (kernel, Memory Management, Power, ...)
        // is read at boot.
        Action::Reg { key, .. }
            if key
                .to_ascii_lowercase()
                .contains(r"\control\session manager\environment") =>
        {
            "a new logon (a reboot for services)"
        }
        Action::Reg { key, .. }
            if key
                .to_ascii_lowercase()
                .contains(r"\control\session manager") =>
        {
            "a reboot"
        }
        Action::Reg { key, .. }
            if key
                .to_ascii_lowercase()
                .contains(r"\multimedia\systemprofile") =>
        {
            "a restart of the MMCSS service (or a reboot)"
        }
        Action::Reg { key, .. } if key.to_ascii_lowercase().contains(r"\services\dasiopacer") => {
            "a reinstall of the filter (or a reboot)"
        }
        Action::Ini { .. } => "Process Lasso's next read of its ini (a restart of Process Lasso)",
        Action::Reg { .. } => "nothing more",
    }
}

/// `text` (an ini file) with the first `key=` line's value set to `value`;
/// a key with no line is added at the end. Line endings are kept.
pub fn set_ini(text: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut done = false;
    let mut out: Vec<String> = text
        .split(eol)
        .map(|l| {
            if !done && l.starts_with(&prefix) {
                done = true;
                format!("{prefix}{value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !done {
        if out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
            out.push(format!("{prefix}{value}"));
            out.push(String::new());
        } else {
            out.push(format!("{prefix}{value}"));
        }
    }
    out.join(eol)
}

/// The value of the first `key=` line of `text`.
pub fn get_ini(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    text.lines().find_map(|l| {
        l.trim_end_matches('\r')
            .strip_prefix(&prefix)
            .map(str::to_string)
    })
}

/// What a `dasio pc-profile check` child process found: the service runs
/// the periodic check in a child at normal priority, off the media CPUs.
/// On dasio's own thread the check held a CPU a media thread's spin tail
/// yielded for a whole quantum: 16 ms gaps at the receivers every 10 min
/// (FOH, dasio#1, 4.11.2026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildCheck {
    /// The profile was read: its problems (none when the PC is as it says).
    Problems(Vec<String>),
    /// The profile was not read (absent or broken): why, as the child said.
    NoProfile(String),
}

/// The child's output: `PROBLEM <text>` lines and a `PC profile ...`
/// summary when the profile was read; otherwise only an error on stderr.
pub fn read_child_check(stdout: &str, stderr: &str) -> ChildCheck {
    let lines = || stdout.lines().map(|l| l.trim_end_matches('\r'));
    if !lines().any(|l| l.starts_with("PC profile ")) {
        return ChildCheck::NoProfile(stderr.trim().to_string());
    }
    ChildCheck::Problems(
        lines()
            .filter_map(|l| l.strip_prefix("PROBLEM "))
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
mod tests;
