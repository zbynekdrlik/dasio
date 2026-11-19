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
//! boot.
//!
//! An entry that would be disabled is refused when disabling it can cut a
//! network path: the Dante NIC or a device it hangs on (its USB hub, host
//! controller, PCI bridge; mbc has no other NIC), any present network
//! adapter (Net class, with or without an address: the remote path, a VLAN
//! adapter over the NIC, a VF under a synthetic adapter), a device such an
//! adapter hangs on or one below it, and a device whose removal takes one
//! of them down (its children and removal relations, as Windows removes
//! them). Without the NIC's device path nothing is disabled at all; an
//! entry whose relations are not read is never disabled.
//!
//! The portable part (validation, the check, the plan, the guard) is here;
//! reading and disabling the device is [`windows`](super::windows).

use serde::Deserialize;

use std::cell::OnceCell;
use std::collections::HashSet;

use super::Problem;

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

/// A present network adapter (Net class): its instance path and its
/// parents', nearest first, up to the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub id: String,
    pub parents: Vec<String>,
}

/// What `[[device]]` reads of the PC's devices (cfgmgr32 on Windows).
pub trait Devices {
    /// The Dante NIC's device instance path, then each of its parents' up
    /// to the root (`[[device]]` never disables one); empty when the NIC or
    /// a parent was not read.
    fn nic_chain(&self) -> Vec<String>;
    /// The device with instance path `id`; `Ok(None)` when the PC has no
    /// such device (no Enum key).
    fn device(&self, id: &str) -> Result<Option<DevState>, String>;
    /// Every present network adapter (Net class, whatever its address or
    /// link), with its parents.
    fn net_adapters(&self) -> Result<Vec<Adapter>, String>;
    /// The parents of device `id`, nearest first, up to the root.
    fn parents(&self, id: &str) -> Result<Vec<String>, String>;
    /// What stops with device `id` when it stops, one step: its children
    /// and its removal relations; empty for a device not connected.
    fn removes(&self, id: &str) -> Result<Vec<String>, String>;
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
    // PnP's own charset (printable ASCII, no space, no comma): the guard
    // compares paths ASCII case-blind, which is then exact.
    if !id.chars().all(|c| c.is_ascii_graphic() && c != ',') {
        return fail(
            "give the device instance path as Device Manager shows it: printable ASCII, no spaces or commas",
        );
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

/// Most devices the guard follows down from one entry (its subtree and
/// removal relations, theirs too): far more than a PC has; past it the
/// entry counts as unread.
pub const MAX_REACH: usize = 4096;

/// What the guard says of an entry that would be disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Off every network path: it may be disabled.
    Free,
    /// What it is to a network path: never disabled.
    Refused(String),
    /// A relation was not read: not disabled (fail closed).
    Unread(String),
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The adapter that stops when `id` stops: `id` is the adapter or one of
/// its parents.
fn taken<'a>(adapters: &'a [Adapter], id: &str) -> Option<&'a Adapter> {
    adapters
        .iter()
        .find(|a| same(&a.id, id) || a.parents.iter().any(|p| same(p, id)))
}

/// The guard for `id`: whether disabling it can cut a network path.
/// `connected` is false only for a device read as not connected, which
/// nothing hangs on and whose disable stops nothing now.
fn judge(
    id: &str,
    connected: bool,
    chain: &[String],
    net: &Result<Vec<Adapter>, String>,
    d: &dyn Devices,
) -> Verdict {
    match chain.iter().position(|c| same(c, id)) {
        Some(0) => return Verdict::Refused("the Dante NIC".into()),
        Some(_) => return Verdict::Refused("a device the Dante NIC hangs on".into()),
        None => {}
    }
    let adapters = match net {
        Ok(a) => a.as_slice(),
        Err(err) => {
            return Verdict::Unread(format!("the network adapters were not read ({err})"));
        }
    };
    if let Some(a) = taken(adapters, id) {
        return Verdict::Refused(if same(&a.id, id) {
            "a network adapter".into()
        } else {
            format!("a device the network adapter {} hangs on", a.id)
        });
    }
    if !connected {
        return Verdict::Free;
    }
    let parents = match d.parents(id) {
        Ok(p) => p,
        Err(err) => return Verdict::Unread(format!("its parents were not read ({err})")),
    };
    if let Some(a) = adapters
        .iter()
        .find(|a| parents.iter().any(|p| same(p, &a.id)))
    {
        return Verdict::Refused(format!("a device below the network adapter {}", a.id));
    }
    // What stops with it: its subtree and removal relations, and theirs,
    // as Windows removes them. `seen` holds them upper case (one lookup
    // each, also in the media process's check).
    let mut reach = vec![id.to_string()];
    let mut seen = HashSet::from([id.to_ascii_uppercase()]);
    for at in 0..MAX_REACH {
        let Some(x) = reach.get(at) else {
            return Verdict::Free;
        };
        let next = match d.removes(x) {
            Ok(n) => n,
            Err(err) => {
                return Verdict::Unread(format!("what stops with {x} was not read ({err})"));
            }
        };
        for y in next {
            if !seen.insert(y.to_ascii_uppercase()) {
                continue;
            }
            // The Dante NIC's own path first: it is guarded also when the
            // adapter list misses it.
            if chain.iter().any(|c| same(c, &y)) {
                return Verdict::Refused(format!(
                    "a device whose removal stops the Dante NIC ({y} stops with it)"
                ));
            }
            if let Some(a) = taken(adapters, &y) {
                return Verdict::Refused(format!(
                    "a device whose removal stops the network adapter {} ({y} stops with it)",
                    a.id
                ));
            }
            reach.push(y);
        }
    }
    Verdict::Unread(format!("more than {MAX_REACH} devices stop with it"))
}

/// One entry against the PC.
enum Seen {
    /// Off now and at every boot, or missing with `absent_ok`.
    Fine,
    /// Not on this PC.
    Missing,
    /// It would be disabled: how it reads, and the guard's verdict.
    Would { found: String, verdict: Verdict },
}

/// The one judge [`check`] and [`plan`] share: an entry that would be
/// disabled goes through the guard. The network adapters are read once,
/// and only for such an entry.
fn see(
    e: &DeviceEntry,
    chain: &[String],
    net: &OnceCell<Result<Vec<Adapter>, String>>,
    d: &dyn Devices,
) -> Seen {
    let (found, connected, unread_state) = match d.device(&e.instance_id) {
        Ok(Some(s)) if s.off() => return Seen::Fine,
        Ok(Some(s)) => (s.describe(), s.present, false),
        Ok(None) if e.absent_ok => return Seen::Fine,
        Ok(None) => return Seen::Missing,
        // An unread state may be a connected device: the guard reads all,
        // and the device is never disabled (fail closed, review of PR #196).
        Err(err) => (format!("unreadable ({err})"), true, true),
    };
    let net = net.get_or_init(|| d.net_adapters());
    let verdict = match judge(&e.instance_id, connected, chain, net, d) {
        Verdict::Free if unread_state => Verdict::Unread("its state was not read".into()),
        v => v,
    };
    Seen::Would { found, verdict }
}

/// Each entry against the PC: one problem (WARN, never must-have) for a
/// device that is not off, one for a missing one (unless `absent_ok`), one
/// for an unreadable one, one for an entry the guard refuses, and one for
/// an entry it cannot judge (the state with why it is not disabled).
/// Without the NIC's path, one problem for them all (and [`plan`] disables
/// nothing).
pub fn check(entries: &[DeviceEntry], d: &dyn Devices) -> Vec<Problem> {
    if entries.is_empty() {
        return Vec::new();
    }
    let chain = d.nic_chain();
    if chain.is_empty() {
        return vec![Problem {
            what: format!("the {} [[device]] entries", entries.len()),
            found: "not checked: the Dante NIC's device path was not read".into(),
            want: "the Dante NIC's device and its parents, so no entry can cut it off".into(),
            why: "no device is disabled without them (dasio#181)".into(),
            must: false,
        }];
    }
    let net = OnceCell::new();
    let mut out = Vec::new();
    for e in entries {
        let problem = |found: String, why: String| Problem {
            what: format!("device {} ({})", e.name, e.instance_id),
            found,
            want: "off".into(),
            why,
            must: false,
        };
        match see(e, &chain, &net, d) {
            Seen::Fine => {}
            Seen::Missing => out.push(problem("not on this PC".into(), e.why.clone())),
            Seen::Would { found, verdict } => out.push(match verdict {
                Verdict::Free => problem(found, e.why.clone()),
                Verdict::Refused(it) => Problem {
                    want: "an entry for another device".into(),
                    ..problem(
                        it,
                        "never disabled by the profile: it cuts the PC off (dasio#181)".into(),
                    )
                },
                Verdict::Unread(err) => {
                    problem(format!("{found}; not disabled: {err}"), e.why.clone())
                }
            }),
        }
    }
    out
}

/// The instance paths `apply` disables: every device that is there and not
/// off, each only when the guard finds it off every network path; nothing
/// without the NIC's path. A device whose state is unread goes through the
/// guard too and is never planned (fail closed).
pub fn plan(entries: &[DeviceEntry], d: &dyn Devices) -> Vec<String> {
    let chain = d.nic_chain();
    if chain.is_empty() {
        return Vec::new();
    }
    let net = OnceCell::new();
    entries
        .iter()
        .filter(|e| {
            matches!(
                see(e, &chain, &net, d),
                Seen::Would {
                    verdict: Verdict::Free,
                    ..
                }
            )
        })
        .map(|e| e.instance_id.clone())
        .collect()
}
