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
//! boot. An entry that would cut a network path is refused: the Dante
//! NIC, any network adapter that is up or has an address (the remote path,
//! e.g. dsg's LAN VLAN adapter; dsg's SoundGrid port, up with no IPv4; an
//! adapter a virtual one rides on), a device they hang on (USB hub, host
//! controller, PCI bridge) or one hanging below them; disabling it cuts a
//! remote PC off (mbc has no other NIC). Without the NIC's path or the
//! adapters, nothing is disabled at all.
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
    // Bus, device and instance, none empty: a bare bus or hardware id names
    // no one device, and a stray `\` never matches Device Manager's path.
    if id.split('\\').count() < 3 || id.split('\\').any(str::is_empty) {
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

/// What `[[device]]` never disables, as the reader found it.
struct Guard {
    /// The Dante NIC's path, then its parents'.
    nic: Vec<String>,
    /// Each network adapter that is up or has an address (the PC's network
    /// paths, the remote one among them): its path, then its parents'.
    live: Vec<Vec<String>>,
}

impl Guard {
    /// Reads it; `Err` is the fail-closed problem's `found`: without it no
    /// device is checked or disabled.
    fn read(r: &dyn Reader) -> Result<Guard, String> {
        let nic = r.nic_chain();
        if nic.is_empty() {
            return Err("not checked: the Dante NIC's device path was not read".into());
        }
        let live = r
            .live()
            .map_err(|e| format!("not checked: the PC's network adapters were not read ({e})"))?;
        Ok(Guard { nic, live })
    }

    /// What `id` is to a network path: `Some` when disabling it cuts one.
    fn on_path(&self, id: &str) -> Option<&'static str> {
        let at = |chain: &[String]| chain.iter().position(|c| c.eq_ignore_ascii_case(id));
        match at(&self.nic) {
            Some(0) => return Some("the Dante NIC"),
            Some(_) => return Some("a device the Dante NIC hangs on"),
            None => {}
        }
        match self.live.iter().filter_map(|c| at(c.as_slice())).min() {
            Some(0) => Some("a network adapter that is up or has an address (a path of the PC)"),
            Some(_) => Some("a device a live network adapter hangs on"),
            None => None,
        }
    }

    /// Whether a device with these parents hangs below the Dante NIC or a
    /// live adapter.
    fn below(&self, parents: &[String]) -> bool {
        let adapters = std::iter::once(&self.nic).chain(&self.live);
        adapters
            .filter_map(|c| c.first())
            .any(|a| parents.iter().any(|p| p.eq_ignore_ascii_case(a)))
    }
}

/// What the profile makes of one entry.
#[derive(Debug, PartialEq, Eq)]
enum Judged {
    /// Off, or missing with `absent_ok`.
    Fine,
    /// Never disabled: it would cut a network path.
    Refused(&'static str),
    /// Not on the PC.
    Missing,
    /// Not read (the device or its parents); never disabled.
    Unreadable(String),
    /// There and not off: `apply` disables it.
    On(String),
}

fn judge(e: &DeviceEntry, r: &dyn Reader, g: &Guard) -> Judged {
    if let Some(it) = g.on_path(&e.instance_id) {
        return Judged::Refused(it);
    }
    let s = match r.device(&e.instance_id) {
        Ok(Some(s)) if s.off() => return Judged::Fine,
        Ok(Some(s)) => s,
        Ok(None) if e.absent_ok => return Judged::Fine,
        Ok(None) => return Judged::Missing,
        Err(err) => return Judged::Unreadable(err),
    };
    match r.parents(&e.instance_id) {
        Ok(p) if g.below(&p) => {
            Judged::Refused("a device below the Dante NIC or a live network adapter")
        }
        Ok(_) => Judged::On(s.describe()),
        Err(err) => Judged::Unreadable(format!("its parents: {err}")),
    }
}

/// Each entry against the PC: one problem (WARN, never must-have) for a
/// device that is not off, one for a missing one (unless `absent_ok`), one
/// for an unreadable one, and one for an entry that would cut a network
/// path (the Dante NIC, a live network adapter, a device they hang on
/// or that hangs below them). Without the NIC's path or the adapters, one
/// problem for them all (and [`plan`] disables nothing).
pub fn check(entries: &[DeviceEntry], r: &dyn Reader) -> Vec<Problem> {
    if entries.is_empty() {
        return Vec::new();
    }
    let g = match Guard::read(r) {
        Ok(g) => g,
        Err(found) => {
            return vec![Problem {
                what: format!("the {} [[device]] entries", entries.len()),
                found,
                want: "the Dante NIC's device, every live network adapter and their parents, so no entry can cut the PC off".into(),
                why: "no device is disabled without them (dasio#181)".into(),
                must: false,
            }];
        }
    };
    let mut out = Vec::new();
    for e in entries {
        let problem = |found: String, want: &str, why: String| Problem {
            what: format!("device {} ({})", e.name, e.instance_id),
            found,
            want: want.into(),
            why,
            must: false,
        };
        match judge(e, r, &g) {
            Judged::Fine => {}
            Judged::Refused(it) => out.push(problem(
                it.into(),
                "an entry for another device",
                "never disabled by the profile: it cuts the PC off (dasio#181)".into(),
            )),
            Judged::Missing => out.push(problem("not on this PC".into(), "off", e.why.clone())),
            Judged::Unreadable(err) => {
                out.push(problem(format!("unreadable ({err})"), "off", e.why.clone()))
            }
            Judged::On(found) => out.push(problem(found, "off", e.why.clone())),
        }
    }
    out
}

/// The instance paths `apply` disables: every device that is there, read
/// and not off, and on no network path; nothing without the guard.
pub fn plan(entries: &[DeviceEntry], r: &dyn Reader) -> Vec<String> {
    let Ok(g) = Guard::read(r) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|e| matches!(judge(e, r, &g), Judged::On(_)))
        .map(|e| e.instance_id.clone())
        .collect()
}
