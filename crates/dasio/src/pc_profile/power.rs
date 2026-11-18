//! Power-plan values of the active plan (dasio#128): `apply` sets them
//! with powercfg, never with a plain registry write. A value the active
//! scheme never stored is not created by a write under its key: on mbc,
//! 16.11.2026, three of the baseline's values (processor maximum, PCIe
//! link state power management, hibernate) stayed absent after
//! install.ps1's apply, and `powercfg /setacvalueindex scheme_current
//! <subgroup> <setting> <value>` + `powercfg /setactive scheme_current`
//! set them. This part decides and builds the commands; `windows.rs` runs
//! them.

use super::{Action, Value};

/// The active power plan's key as a profile writes it.
pub const ACTIVE_SCHEME: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\{active_scheme}";

/// One value of the active plan: its subgroup and setting GUIDs, and
/// whether it is the plugged-in (AC) or the battery (DC) index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerSetting {
    pub subgroup: String,
    pub setting: String,
    pub ac: bool,
}

/// `key`\`name` as a value of the active plan:
/// `{active_scheme}\<subgroup GUID>\<setting GUID>` with `ACSettingIndex`
/// or `DCSettingIndex`. Anything else (another value name or depth, a part
/// that is no GUID, a literal scheme GUID) is `None`: a registry entry.
pub fn power_setting(key: &str, name: &str) -> Option<PowerSetting> {
    let ac = match name {
        "ACSettingIndex" => true,
        "DCSettingIndex" => false,
        _ => return None,
    };
    let rest = key.strip_prefix(ACTIVE_SCHEME)?.strip_prefix('\\')?;
    let (subgroup, setting) = rest.split_once('\\')?;
    if !is_guid(subgroup) || !is_guid(setting) {
        return None;
    }
    Some(PowerSetting {
        subgroup: subgroup.to_string(),
        setting: setting.to_string(),
        ac,
    })
}

/// A GUID without braces: 8-4-4-4-12 hex digits.
fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The powercfg commands (arguments) that set `a`, when it is a DWORD
/// value of the active plan: the index, then `/setactive` so the running
/// plan loads it. `None` for every other action (a registry or ini write).
pub fn powercfg_commands(a: &Action) -> Option<Vec<Vec<String>>> {
    let Action::Reg {
        key,
        name,
        value: Value::Dword(v),
    } = a
    else {
        return None;
    };
    let p = power_setting(key, name)?;
    let set = if p.ac {
        "/setacvalueindex"
    } else {
        "/setdcvalueindex"
    };
    Some(vec![
        vec![
            set.to_string(),
            "scheme_current".to_string(),
            p.subgroup,
            p.setting,
            v.to_string(),
        ],
        vec!["/setactive".to_string(), "scheme_current".to_string()],
    ])
}

/// What `apply` does for `a`, for its log line: the powercfg commands of a
/// power-plan value, else the registry value or the ini line it writes.
pub fn describe(a: &Action) -> String {
    if let Some(cmds) = powercfg_commands(a) {
        return cmds
            .iter()
            .map(|c| format!("powercfg {}", c.join(" ")))
            .collect::<Vec<_>>()
            .join(" + ");
    }
    match a {
        Action::Reg { key, name, value } => format!("registry {key}\\{name} = {value:?}"),
        Action::Ini { file, key, value } => format!("{file} {key}={value}"),
        Action::Remove(r) => r.describe(),
        Action::Disable(id) => format!("device {id} off"),
    }
}
