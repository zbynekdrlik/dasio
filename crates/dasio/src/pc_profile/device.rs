//! `[[device]]` entries (dasio#181, owner 18.11.2026): a device the PC's
//! job does not need, held off ("extreme production mode": every device,
//! driver and service the PC does not need is off and guarded). Named by
//! its device instance path (Device Manager's "Device instance path"), so
//! entries are site data: the same hardware has another path on another
//! PC.
//!
//! The check reads the device's live state (its problem code: 22 is
//! "disabled") and whether it stays off across a boot (CONFIGFLAG_DISABLED
//! in its Enum key's `ConfigFlags`); `apply` disables it now and for every
//! boot. An entry naming the Dante NIC itself is refused: disabling it
//! cuts a remote PC off (mbc has no other NIC).
//!
//! The portable part (validation, the check, the plan) is here; reading
//! and disabling the device is [`windows`](super::windows).

use serde::Deserialize;

use super::{Problem, Reader};

/// A device that must be off.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceEntry {
    /// Its device instance path, e.g. `PCI\VEN_8086&DEV_54C8&...\3&...&FB`
    /// (case ignored).
    pub instance_id: String,
    /// What it is, as the check shows it (e.g. "onboard audio").
    pub name: String,
    /// Why the PC does not need it.
    pub why: String,
    /// The device may be missing altogether (no Enum key: removed, or a
    /// USB device never plugged into this PC).
    #[serde(default)]
    pub absent_ok: bool,
}

/// CM_PROB_DISABLED: the device is disabled.
pub const PROBLEM_DISABLED: u32 = 22;

/// What the PC shows of one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevState {
    /// It is connected (a devnode exists, not only its Enum key).
    pub present: bool,
    /// Its problem code while present; `None` without a problem.
    pub problem: Option<u32>,
    /// CONFIGFLAG_DISABLED is set: it stays off across a boot.
    pub config_disabled: bool,
}

impl DevState {
    /// Off now and at every boot.
    pub fn off(&self) -> bool {
        self.config_disabled && (!self.present || self.problem == Some(PROBLEM_DISABLED))
    }

    /// How the check shows a state that is not [`off`](Self::off).
    pub fn describe(&self) -> String {
        match (self.present, self.problem, self.config_disabled) {
            (false, _, false) => "not connected, on when connected".into(),
            (true, Some(PROBLEM_DISABLED), false) => {
                "off until the next boot (ConfigFlags does not hold it)".into()
            }
            (true, problem, held) => {
                let mut s = String::from("on");
                if let Some(p) = problem {
                    s.push_str(&format!(" (problem {p})"));
                }
                if held {
                    s.push_str(", off only from the next boot");
                }
                s
            }
            (false, _, true) => "off".into(),
        }
    }
}

/// What an entry must hold; the error names the entry and the field.
pub fn validate(e: &DeviceEntry) -> Result<(), String> {
    let fail = |what: &str| Err(format!("device {:?}: {what}", e.instance_id));
    let id = e.instance_id.trim();
    if id != e.instance_id || id.is_empty() {
        return fail("give the device instance path, without spaces around it");
    }
    if id.to_ascii_uppercase().starts_with("HKLM") {
        return fail(r"give the device instance path (e.g. PCI\VEN_...\...), not a registry key");
    }
    // Bus, device and instance: a bare bus or hardware id names no one device.
    if id.split('\\').filter(|p| !p.is_empty()).count() < 3 {
        return fail(r"give the whole device instance path: <bus>\<device>\<instance>");
    }
    if e.name.trim().is_empty() {
        return fail("name must not be empty");
    }
    if e.why.trim().is_empty() {
        return fail("why must not be empty");
    }
    Ok(())
}

/// Whether `e` names the Dante NIC (`nic`, its instance path).
fn is_nic(e: &DeviceEntry, nic: Option<&str>) -> bool {
    nic.is_some_and(|n| n.eq_ignore_ascii_case(&e.instance_id))
}

/// Each entry against the PC: one problem (WARN, never must-have) for a
/// device that is not off, one for a missing one (unless `absent_ok`), one
/// for an unreadable one, and one for an entry naming the Dante NIC.
pub fn check(entries: &[DeviceEntry], r: &dyn Reader) -> Vec<Problem> {
    let nic = r.nic_instance_id();
    let mut out = Vec::new();
    for e in entries {
        let what = format!("device {} ({})", e.name, e.instance_id);
        let problem = |found: String, why: String| Problem {
            what: what.clone(),
            found,
            want: "off".into(),
            why,
            must: false,
        };
        if is_nic(e, nic.as_deref()) {
            out.push(Problem {
                want: "an entry for another device".into(),
                ..problem(
                    "the Dante NIC".into(),
                    "never disabled by the profile: it cuts the PC off (dasio#181)".into(),
                )
            });
            continue;
        }
        match r.device(&e.instance_id) {
            Ok(Some(s)) if s.off() => {}
            Ok(Some(s)) => out.push(problem(s.describe(), e.why.clone())),
            Ok(None) if e.absent_ok => {}
            Ok(None) => out.push(problem("not on this PC".into(), e.why.clone())),
            Err(err) => out.push(problem(format!("unreadable ({err})"), e.why.clone())),
        }
    }
    out
}

/// The instance paths `apply` disables: every device that is there and not
/// off, never the Dante NIC; an unreadable one too (the disable says why
/// it fails).
pub fn plan(entries: &[DeviceEntry], r: &dyn Reader) -> Vec<String> {
    let nic = r.nic_instance_id();
    entries
        .iter()
        .filter(|e| !is_nic(e, nic.as_deref()))
        .filter(|e| match r.device(&e.instance_id) {
            Ok(Some(s)) => !s.off(),
            Ok(None) => false,
            Err(_) => true,
        })
        .map(|e| e.instance_id.clone())
        .collect()
}
