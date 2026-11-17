//! The PC profile (dasio#5, owner 4.11.2026): the Windows settings a PC
//! must keep for dasio, declared once in a private TOML file on the PC
//! (`C:\ProgramData\dasio\pc-profile.toml`, site data, never in the repo),
//! so a Windows upgrade or a reinstall never means finding them again.
//! The service checks it at start and every [`CHECK_EVERY_S`], in a child
//! process off the media CPUs ([`ChildCheck`]), and reports what differs;
//! `dasio pc-profile apply` sets what differs.
//!
//! Eight kinds of entries:
//! - `[[registry]]`: one value under `HKLM\...` (a DWORD, a string, a
//!   list of strings (REG_MULTI_SZ, e.g. a NIC's `SubnetMask`), or binary
//!   as hex). `{nic_class}` and `{nic_device}` in a key stand for
//!   the Dante NIC's class key and its Enum `Device Parameters` key, found
//!   from `nic_guid`.
//! - `[[ini]]`: one `key=value` line of an ini file (Process Lasso's).
//! - `[[process]]`: what a running process must have (affinity, priority
//!   class), to see that the tool which sets it (Lasso) did; checked only,
//!   never applied, and skipped while the process does not run.
//! - `[[disk]]`: a floor of free space on a drive, so a filling disk shows
//!   as a problem before it hurts (FOH's C: had 26.5 GB left, owner
//!   9.11.2026, dasio#1); checked only, never applied.
//! - `[[bcd]]`: one setting of the boot entry Windows runs from
//!   (`bcdedit /enum {current}`, e.g. `disabledynamictick Yes`); checked
//!   only, never applied: a change takes a reboot.
//! - `[[task]]`: one Task Scheduler task, whether it is on and its triggers
//!   (`schtasks /query /tn <name> /xml`, e.g. FOH's fleetbackup moved from
//!   every 2 h to daily 04:00); checked only, never applied: the task is
//!   another tool's install.
//! - `[[absent]]`: a program, service or task that must not exist (Edge
//!   and its updater, dasio#128); `apply` removes it ([`absent`]).
//! - `[[nic_driver]]`: the driver package a family of Dante NICs must run
//!   (by hardware ID, dasio#128); checked only, `install.ps1 -NicDriver`
//!   installs it ([`nic_driver`]).
//!
//! Besides, the service holds the services Windows turns back on by itself
//! (UsoSvc, dasio#158) off before each check ([`hold`]).
//!
//! Every PC is held to the built-in baseline merged with its own profile
//! ([`baseline`](self::baseline), dasio#128); the baseline's CPU masks are
//! computed from the PC's CPU count ([`layout`](self::layout)), and its
//! `must = true` items stop dasio when missing.
//!
//! This part is portable and tested everywhere; reading and writing the
//! PC is [`windows`](self::windows).

use serde::Deserialize;

pub mod absent;
mod baseline;
pub mod hold;
pub mod layout;
pub mod nic_driver;
mod os_text;
pub(crate) mod power;
#[cfg(windows)]
pub mod windows;

pub use absent::{AbsentEntry, Removal};
pub use baseline::{
    BASELINE, MUST_HAVE, baseline, baseline_only, load, load_merged, merge, must_have_failure,
};
pub use layout::{Layout, resolve};
pub use nic_driver::NicDriverEntry;
pub use os_text::{parse_bcdedit, parse_task_xml};
pub use power::{PowerSetting, describe, power_setting, powercfg_commands};

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
    /// How many dasio instances the PC runs, for the CPU layout
    /// ([`layout`]); 1 when not given.
    #[serde(default)]
    pub instances: Option<usize>,
    #[serde(default)]
    pub registry: Vec<RegEntry>,
    #[serde(default)]
    pub ini: Vec<IniEntry>,
    #[serde(default)]
    pub process: Vec<ProcEntry>,
    #[serde(default)]
    pub disk: Vec<DiskEntry>,
    #[serde(default)]
    pub bcd: Vec<BcdEntry>,
    #[serde(default)]
    pub task: Vec<TaskEntry>,
    /// What must not exist on the PC ([`absent`], dasio#128).
    #[serde(default)]
    pub absent: Vec<AbsentEntry>,
    /// The driver package each Dante NIC family must run ([`nic_driver`],
    /// dasio#128).
    #[serde(default)]
    pub nic_driver: Vec<NicDriverEntry>,
}

impl Profile {
    /// How many entries it holds, of every kind.
    pub fn entries(&self) -> usize {
        self.registry.len()
            + self.ini.len()
            + self.process.len()
            + self.disk.len()
            + self.bcd.len()
            + self.task.len()
            + self.absent.len()
            + self.nic_driver.len()
    }
}

/// An expected registry value: exactly one of `dword`, `string`,
/// `multi_sz`, `binary`.
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
    /// A REG_MULTI_SZ, e.g. `["255.255.255.0"]`.
    #[serde(default)]
    pub multi_sz: Option<Vec<String>>,
    /// Hex bytes, e.g. `"0f00000000000000"`.
    #[serde(default)]
    pub binary: Option<String>,
    /// A REG_QWORD.
    #[serde(default)]
    pub qword: Option<u64>,
    /// An absent value (or key) is fine too: e.g. the `Start` of a service
    /// that may be uninstalled. A value other than the wanted one is still
    /// a problem, and nothing is created for an absent one.
    #[serde(default)]
    pub absent_ok: bool,
    /// Only when the Dante NIC's driver service is this one (its Enum
    /// key's `Service`, e.g. `rt640x64`, case ignored): a NIC family's
    /// own settings in the baseline (dasio#128). Unset: always.
    #[serde(default)]
    pub nic_service: Option<String>,
    /// Checked, never set by `apply`: a value that reports a state (e.g.
    /// a driver's `DriverVersion`), which only installing the thing fixes.
    #[serde(default)]
    pub check_only: bool,
    /// The value comes from the PC's CPU layout instead of being given:
    /// one of [`layout::COMPUTED`] (dasio#128).
    #[serde(default)]
    pub computed: Option<String>,
    /// dasio does not run without it (dasio#128): the service ends, the
    /// check exits 2, `install.ps1` refuses.
    #[serde(default)]
    pub must: bool,
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

/// An expected setting of the running boot entry (`bcdedit /enum
/// {current}`): its element name and value, both compared without case.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BcdEntry {
    /// The element as bcdedit lists it, e.g. `disabledynamictick`.
    pub name: String,
    /// Its value as bcdedit lists it, e.g. `Yes`.
    pub value: String,
    /// dasio does not run without it (dasio#128), as [`RegEntry::must`].
    #[serde(default)]
    pub must: bool,
    #[serde(default)]
    pub why: String,
}

/// An expected scheduled task.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskEntry {
    /// The task's path as schtasks shows it, e.g. `\fleetbackup-main`.
    pub name: String,
    /// Whether the task is on; unchecked when not given.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Its triggers in order, as [`parse_task_xml`] describes them, e.g.
    /// `["daily 04:00"]`; unchecked when not given.
    #[serde(default)]
    pub triggers: Option<Vec<String>>,
    /// An absent task is fine too (the tool may be uninstalled).
    #[serde(default)]
    pub absent_ok: bool,
    #[serde(default)]
    pub why: String,
}

/// A scheduled task as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskState {
    pub enabled: bool,
    pub triggers: Vec<String>,
}

/// Bytes in the GB the profile and its problems use.
const GB: u64 = 1 << 30;

/// A registry value as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Dword(u32),
    /// A REG_QWORD (e.g. a QoS policy's ThrottleRate, dasio#128).
    Qword(u64),
    String(String),
    MultiSz(Vec<String>),
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
    /// The running boot entry's settings, name and value
    /// ([`parse_bcdedit`] of `bcdedit /enum {current}`).
    fn bcd(&self) -> Result<Vec<(String, String)>, String>;
    /// The scheduled task `name` ([`parse_task_xml`] of its XML); `Ok(None)`
    /// when there is no such task.
    fn task(&self, name: &str) -> Result<Option<TaskState>, String>;
    /// The Dante NIC's driver service (e.g. `rt640x64`); `None` when the
    /// NIC or its service was not found.
    fn nic_service(&self) -> Option<String>;
    /// What of `e`'s thing the PC has ([`absent`]): the exe's path, the
    /// service's name, each task the pattern matches by its full path;
    /// empty when there is none.
    fn absent(&self, e: &AbsentEntry) -> Result<Vec<String>, String>;
    /// The Dante NIC's PnP hardware IDs (its Enum key's `HardwareID`),
    /// most specific first; empty when the NIC was not found.
    fn nic_hardware_ids(&self) -> Vec<String>;
}

/// Whether `e` applies on the PC `r` reads: an entry for one NIC driver
/// family only where the Dante NIC runs that driver.
fn applies(e: &RegEntry, r: &dyn Reader) -> bool {
    match &e.nic_service {
        None => true,
        Some(want) => r
            .nic_service()
            .is_some_and(|have| have.eq_ignore_ascii_case(want)),
    }
}

/// One setting that is not as the profile says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub what: String,
    pub found: String,
    pub want: String,
    pub why: String,
    /// A must-have item (dasio#128): its text starts with [`MUST_HAVE`].
    pub must: bool,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.must {
            f.write_str(MUST_HAVE)?;
        }
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

/// The dasio instances the site profile at `path` names (`instances`,
/// default 1; no profile: 1), for the CPU layout (dasio#128, #120). From
/// the site file alone, so a layout whose placeholders do not fit still
/// reads.
pub fn site_instances(path: &std::path::Path) -> Result<usize, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map(|p| p.instances.unwrap_or(1)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(1),
        Err(e) => Err(e.to_string()),
    }
}

/// Reads a PC's profile; the error names what is wrong in it.
pub fn parse(text: &str) -> Result<Profile, String> {
    parse_with(text, true)
}

fn parse_with(text: &str, need_nic_guid: bool) -> Result<Profile, String> {
    let p: Profile = toml::from_str(text).map_err(|e| e.to_string())?;
    if p.instances == Some(0) {
        return Err("instances must be 1 or more".into());
    }
    for r in &p.registry {
        if let Some(c) = &r.computed {
            if !layout::COMPUTED.contains(&c.as_str()) {
                return Err(format!(
                    "registry {}\\{}: computed {c} is not one of {:?}",
                    r.key,
                    r.name,
                    layout::COMPUTED
                ));
            }
            if want_reg(r).is_some() {
                return Err(format!(
                    "registry {}\\{}: give computed or a value, not both",
                    r.key, r.name
                ));
            }
        } else if want_reg(r).is_none() {
            return Err(format!(
                "registry {}\\{}: give exactly one of dword, string, binary (binary as hex), multi_sz, qword",
                r.key, r.name
            ));
        }
        if need_nic_guid
            && (r.key.contains("{nic_class}") || r.key.contains("{nic_device}"))
            && p.nic_guid.is_none()
        {
            return Err(format!("registry {}: needs nic_guid", r.key));
        }
    }
    for e in &p.process {
        if let Some(a) = &e.affinity
            && !layout::affinity_ok(a)
        {
            return Err(format!(
                "process {}: affinity {a} is not a hex mask, {{system}}, {{dasio:N}} or {{daw}}",
                e.exe
            ));
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
    for b in &p.bcd {
        if b.name.is_empty() || b.name.contains(char::is_whitespace) {
            return Err(format!(
                "bcd {:?}: give the element as bcdedit lists it, one word, e.g. disabledynamictick",
                b.name
            ));
        }
        if b.value.trim().is_empty() {
            return Err(format!("bcd {}: value must not be empty", b.name));
        }
    }
    for e in &p.task {
        if !e.name.starts_with('\\') {
            return Err(format!(
                "task {:?}: give the path as schtasks shows it, e.g. \\fleetbackup-main",
                e.name
            ));
        }
        if e.enabled.is_none() && e.triggers.is_none() {
            return Err(format!("task {}: give enabled, triggers or both", e.name));
        }
    }
    for e in &p.absent {
        absent::validate(e)?;
    }
    for e in &p.nic_driver {
        nic_driver::validate(e)?;
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
/// `dword`, `string`, `multi_sz`, `binary` (valid hex) is given.
pub fn want_reg(r: &RegEntry) -> Option<Value> {
    match (&r.dword, &r.string, &r.multi_sz, &r.binary, &r.qword) {
        (Some(d), None, None, None, None) => Some(Value::Dword(*d)),
        (None, Some(s), None, None, None) => Some(Value::String(s.clone())),
        (None, None, Some(m), None, None) => Some(Value::MultiSz(m.clone())),
        (None, None, None, Some(b), None) => hex(b).map(Value::Binary),
        (None, None, None, None, Some(q)) => Some(Value::Qword(*q)),
        _ => None,
    }
}

/// A REG_MULTI_SZ's UTF-16 data as its strings: each ends with a NUL, an
/// empty string ends the list (missing terminators are tolerated).
pub fn multi_sz_from_wide(w: &[u16]) -> Vec<String> {
    w.split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// The UTF-16 data Windows stores for a REG_MULTI_SZ of `strings`: each
/// with its NUL, then one more NUL.
pub fn multi_sz_to_wide(strings: &[String]) -> Vec<u16> {
    let mut w: Vec<u16> = strings
        .iter()
        .flat_map(|s| s.encode_utf16().chain(Some(0)))
        .collect();
    w.push(0);
    if strings.is_empty() {
        w.push(0);
    }
    w
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
        Value::Qword(q) => format!("{q} (0x{q:X}, qword)"),
        Value::String(s) => format!("\"{s}\""),
        Value::MultiSz(m) => format!("{m:?}"),
        Value::Binary(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
    }
}

/// Every setting of `p` the PC does not have.
pub fn check(p: &Profile, r: &dyn Reader) -> Vec<Problem> {
    let mut out = Vec::new();
    // NIC-family entries need the NIC: a profile naming one the PC does not
    // have (a wrong GUID, a swapped NIC) says so instead of skipping them.
    if let Some(guid) = &p.nic_guid
        && r.nic_service().is_none()
        && p.registry.iter().any(|e| e.nic_service.is_some())
    {
        out.push(Problem {
            what: format!("the Dante NIC {guid}"),
            found: "not found (or its driver service unreadable)".into(),
            want: "the adapter with that NetCfgInstanceId".into(),
            why: "the baseline's NIC settings need it (dasio#128)".into(),
            must: p.registry.iter().any(|e| e.must && e.nic_service.is_some()),
        });
    }
    for e in &p.registry {
        let Some(want) = want_reg(e) else { continue };
        if !applies(e, r) {
            continue;
        }
        let found = r.reg(&e.key, &e.name);
        let found_text = match &found {
            Ok(Some(v)) if *v == want => continue,
            Ok(None) if e.absent_ok => continue,
            Ok(Some(v)) => show(v),
            Ok(None) => "absent".to_string(),
            Err(err) => format!("unreadable ({err})"),
        };
        out.push(Problem {
            what: format!("registry {}\\{}", e.key, e.name),
            found: found_text,
            want: show(&want),
            why: e.why.clone(),
            must: e.must,
        });
    }
    // A file that cannot be read is one problem, not one per key: a PC
    // without Process Lasso shows it once (dasio#128).
    let mut unreadable: Vec<&str> = Vec::new();
    for e in &p.ini {
        if unreadable.iter().any(|f| f.eq_ignore_ascii_case(&e.file)) {
            continue;
        }
        let found = match r.ini(&e.file, &e.key) {
            Ok(Some(v)) if v == e.value => continue,
            Ok(Some(v)) => format!("\"{v}\""),
            Ok(None) => "absent".to_string(),
            Err(err) => {
                unreadable.push(&e.file);
                let keys = p
                    .ini
                    .iter()
                    .filter(|x| x.file.eq_ignore_ascii_case(&e.file))
                    .count();
                out.push(Problem {
                    what: e.file.clone(),
                    found: format!("unreadable ({err})"),
                    want: format!("the file, with the {keys} key(s) the profile names"),
                    why: e.why.clone(),
                    must: false,
                });
                continue;
            }
        };
        out.push(Problem {
            what: format!("{} {}", e.file, e.key),
            found,
            want: format!("\"{}\"", e.value),
            why: e.why.clone(),
            must: false,
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
            must: false,
        });
    }
    if !p.bcd.is_empty() {
        let found = r.bcd();
        for e in &p.bcd {
            let text = match &found {
                Ok(list) => match list.iter().find(|(n, _)| n.eq_ignore_ascii_case(&e.name)) {
                    Some((_, v)) if v.eq_ignore_ascii_case(e.value.trim()) => continue,
                    Some((_, v)) => format!("\"{v}\""),
                    None => "absent".to_string(),
                },
                Err(err) => format!("unreadable ({err})"),
            };
            out.push(Problem {
                what: format!("bcd {}", e.name),
                found: text,
                want: format!("\"{}\"", e.value.trim()),
                why: e.why.clone(),
                must: e.must,
            });
        }
    }
    for e in &p.task {
        let state = match r.task(&e.name) {
            Ok(Some(s)) => s,
            Ok(None) if e.absent_ok => continue,
            Ok(None) => {
                out.push(Problem {
                    what: format!("task {}", e.name),
                    found: "absent".into(),
                    want: "the task".into(),
                    why: e.why.clone(),
                    must: false,
                });
                continue;
            }
            Err(err) => {
                out.push(Problem {
                    what: format!("task {}", e.name),
                    found: format!("unreadable ({err})"),
                    want: "the task".into(),
                    why: e.why.clone(),
                    must: false,
                });
                continue;
            }
        };
        if let Some(want) = e.enabled
            && state.enabled != want
        {
            let on = |b: bool| if b { "on" } else { "off" };
            out.push(Problem {
                what: format!("task {}", e.name),
                found: on(state.enabled).into(),
                want: on(want).into(),
                why: e.why.clone(),
                must: false,
            });
        }
        if let Some(want) = &e.triggers
            && state.triggers != *want
        {
            out.push(Problem {
                what: format!("task {} triggers", e.name),
                found: format!("{:?}", state.triggers),
                want: format!("{want:?}"),
                why: e.why.clone(),
                must: false,
            });
        }
    }
    out.extend(absent::check(&p.absent, r));
    out.extend(nic_driver::check(&p.nic_driver, r));
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
    /// Removes what an `[[absent]]` entry names (dasio#128).
    Remove(Removal),
}

/// What has to change for the PC to match (processes are only checked:
/// the tool that sets them, Lasso, is configured through its ini). An
/// unreadable value is planned too: the write says why it fails.
pub fn plan(p: &Profile, r: &dyn Reader) -> Vec<Action> {
    let mut out = Vec::new();
    for e in &p.registry {
        let Some(want) = want_reg(e) else { continue };
        if !applies(e, r) || e.check_only {
            continue;
        }
        let fine = match r.reg(&e.key, &e.name) {
            Ok(Some(v)) => v == want,
            Ok(None) => e.absent_ok,
            Err(_) => false,
        };
        if !fine {
            out.push(Action::Reg {
                key: e.key.clone(),
                name: e.name.clone(),
                value: want,
            });
        }
    }
    // A file that cannot be read gets one write (it says why it fails),
    // not one per key (dasio#128).
    let mut unreadable: Vec<&str> = Vec::new();
    for e in &p.ini {
        if unreadable.iter().any(|f| f.eq_ignore_ascii_case(&e.file)) {
            continue;
        }
        let found = r.ini(&e.file, &e.key);
        if found.is_err() {
            unreadable.push(&e.file);
        }
        if !matches!(found, Ok(Some(ref v)) if *v == e.value) {
            out.push(Action::Ini {
                file: e.file.clone(),
                key: e.key.clone(),
                value: e.value.clone(),
            });
        }
    }
    out.extend(absent::plan(&p.absent, r).into_iter().map(Action::Remove));
    out
}

/// What an applied action needs before it takes effect.
pub fn takes_effect(a: &Action) -> &'static str {
    match a {
        Action::Reg { key, .. } if key.starts_with("{nic_") => "a restart of the Dante NIC",
        // apply sets a power-plan value with powercfg and activates the
        // plan (dasio#128).
        a if powercfg_commands(a).is_some() => {
            "nothing more (apply ran powercfg /setactive scheme_current)"
        }
        Action::Reg { key, .. } if key.contains("{active_scheme}") => {
            "powercfg /setactive of the active plan (install.ps1 runs it) or a reboot"
        }
        Action::Reg { key, .. }
            if key
                .to_ascii_lowercase()
                .contains(r"\policies\microsoft\windows\qos") =>
        {
            "gpupdate /target:computer (install.ps1 runs it) or a reboot"
        }
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
        Action::Remove(Removal::Service(_)) => {
            "nothing more (a service still held open goes at the next reboot)"
        }
        Action::Remove(_) => "nothing more",
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

/// The part of `p` a check reads without starting a process: a bridge
/// instance checks in its own process, since a child started while an
/// ASIO driver is loaded makes SoundGrid resync (dasio#106, dasio#120).
/// `[[bcd]]` and `[[task]]` need `bcdedit` / `schtasks`: they are left to
/// `dasio pc-profile check` and the boot-time `pc-profile apply`.
pub fn in_process(p: &Profile) -> Profile {
    Profile {
        bcd: Vec::new(),
        task: Vec::new(),
        ..p.clone()
    }
}

/// The problems of `now` not in `before` (they appeared) and of `before`
/// not in `now` (they were fixed), each in its list's order.
pub fn changes<'a>(before: &'a [String], now: &'a [String]) -> (Vec<&'a str>, Vec<&'a str>) {
    let appeared = now
        .iter()
        .filter(|t| !before.contains(t))
        .map(String::as_str)
        .collect();
    let fixed = before
        .iter()
        .filter(|t| !now.contains(t))
        .map(String::as_str)
        .collect();
    (appeared, fixed)
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
