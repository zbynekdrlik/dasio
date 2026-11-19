//! `[[device]]` entries (dasio#181): a device the PC does not need, held
//! off; never the Dante NIC. The guard's other network paths are
//! [`device_guard`](super::device_guard)'s.

use std::cell::Cell;
use std::collections::HashMap;

use super::*;

pub(super) const AUDIO: &str = r"PCI\VEN_8086&DEV_54C8&SUBSYS_17951025&REV_00\3&11583659&0&FB";
pub(super) const BT: &str = r"USB\VID_8087&PID_0026\5&C99FFEC&0&10";
pub(super) const NIC: &str = r"USB\VID_0BDA&PID_8156\4F13000001";
/// The hub and the host controller mbc's USB Dante NIC hangs on.
pub(super) const HUB: &str = r"USB\ROOT_HUB30\5&1EF46A2D&0&0";
pub(super) const XHCI: &str = r"PCI\VEN_1022&DEV_1639&SUBSYS_387017AA&REV_00\4&2D99F355&0&0441";
pub(super) const ROOT: &str = r"HTREE\ROOT\0";

/// The PC's devices as a test sets them up (paths upper case).
#[derive(Default)]
pub(super) struct Fake {
    /// The Dante NIC's device instance path, then its parents'.
    pub(super) nic_chain: Vec<String>,
    /// Devices by instance path.
    pub(super) devices: HashMap<String, DevState>,
    /// `device()` fails with this.
    pub(super) device_err: Option<String>,
    /// The present network adapters; `net_err` fails their read.
    pub(super) adapters: Vec<Adapter>,
    pub(super) net_err: Option<String>,
    /// How often the adapters were read.
    pub(super) net_reads: Cell<u32>,
    /// Each device's parents (none listed: none).
    pub(super) parents: HashMap<String, Vec<String>>,
    /// What stops with each device (none listed: nothing).
    pub(super) removes: HashMap<String, Vec<String>>,
    /// `parents()` and `removes()` fail for these.
    pub(super) unread: Vec<String>,
    /// Every device stops a new one with it (an endless tree): the n-th
    /// call gives `ROOT\DEEP\n`.
    pub(super) endless: Option<Cell<u32>>,
}

impl Fake {
    fn read(&self, what: &str, id: &str) -> Result<(), String> {
        if self.unread.contains(&id.to_ascii_uppercase()) {
            return Err(format!("{what} {id}: CONFIGRET 13"));
        }
        Ok(())
    }
}

impl Devices for Fake {
    fn nic_chain(&self) -> Vec<String> {
        self.nic_chain.clone()
    }
    fn device(&self, id: &str) -> Result<Option<DevState>, String> {
        if let Some(err) = &self.device_err {
            return Err(err.clone());
        }
        Ok(self.devices.get(&id.to_ascii_uppercase()).copied())
    }
    fn net_adapters(&self) -> Result<Vec<Adapter>, String> {
        self.net_reads.set(self.net_reads.get() + 1);
        match &self.net_err {
            Some(err) => Err(err.clone()),
            None => Ok(self.adapters.clone()),
        }
    }
    fn parents(&self, id: &str) -> Result<Vec<String>, String> {
        self.read("parents", id)?;
        Ok(self
            .parents
            .get(&id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default())
    }
    fn removes(&self, id: &str) -> Result<Vec<String>, String> {
        self.read("removes", id)?;
        if let Some(n) = &self.endless {
            n.set(n.get() + 1);
            return Ok(vec![format!(r"ROOT\DEEP\{}", n.get())]);
        }
        Ok(self
            .removes
            .get(&id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default())
    }
}

pub(super) fn profile(entries: &str) -> Profile {
    parse(&format!("name = \"dsg\"\n{entries}")).unwrap()
}

pub(super) fn entry(id: &str, name: &str) -> String {
    format!("[[device]]\ninstance_id = '{id}'\nname = \"{name}\"\nwhy = \"not needed ({name})\"\n")
}

pub(super) fn state(present: bool, problem: Option<u32>, config_disabled: bool) -> DevState {
    DevState {
        present,
        problem,
        config_disabled,
    }
}

/// A PC with these devices; the Dante NIC is its only network adapter.
pub(super) fn pc(devices: &[(&str, DevState)]) -> Pc {
    Pc {
        dev: Fake {
            devices: devices
                .iter()
                .map(|(id, s)| (id.to_ascii_uppercase(), *s))
                .collect(),
            nic_chain: vec![NIC.into(), HUB.into(), XHCI.into(), ROOT.into()],
            adapters: vec![Adapter {
                id: NIC.into(),
                parents: vec![HUB.into(), XHCI.into(), ROOT.into()],
            }],
            ..Fake::default()
        },
        ..Pc::default()
    }
}

pub(super) const OFF: DevState = DevState {
    present: true,
    problem: Some(22),
    config_disabled: true,
};

#[test]
fn a_profile_with_devices_parses_and_counts_them() {
    let p = profile(&format!(
        "{}{}absent_ok = true\n",
        entry(AUDIO, "audio"),
        entry(BT, "bt")
    ));
    assert_eq!(p.entries(), 2);
    assert_eq!(
        p.device,
        vec![
            DeviceEntry {
                instance_id: AUDIO.into(),
                name: "audio".into(),
                why: "not needed (audio)".into(),
                absent_ok: false,
            },
            DeviceEntry {
                instance_id: BT.into(),
                name: "bt".into(),
                why: "not needed (bt)".into(),
                absent_ok: true,
            },
        ]
    );
}

#[test]
fn an_entry_must_name_one_device_by_its_whole_instance_path() {
    let err = |id: &str, name: &str, why: &str| {
        parse(&format!(
            "name = \"dsg\"\n[[device]]\ninstance_id = '{id}'\nname = \"{name}\"\nwhy = \"{why}\"\n"
        ))
        .unwrap_err()
    };
    assert_eq!(
        err(r"PCI\VEN_8086&DEV_54C8", "a", "b"),
        r#"device "PCI\\VEN_8086&DEV_54C8": give the whole device instance path: <bus>\<device>\<instance>"#
    );
    assert_eq!(
        err(r"HKLM\SYSTEM\CurrentControlSet\Enum\PCI\X\Y", "a", "b"),
        r#"device "HKLM\\SYSTEM\\CurrentControlSet\\Enum\\PCI\\X\\Y": give the device instance path (e.g. PCI\VEN_...\...), not a registry key"#
    );
    assert_eq!(
        err(&format!(" {AUDIO}"), "a", "b"),
        format!(
            "device {:?}: give the device instance path, without spaces around it",
            format!(" {AUDIO}")
        )
    );
    assert_eq!(
        err("", "a", "b"),
        r#"device "": give the device instance path, without spaces around it"#
    );
    assert_eq!(
        err(AUDIO, " ", "b"),
        format!("device {AUDIO:?}: name must not be empty")
    );
    assert_eq!(
        err(AUDIO, "a", " "),
        format!("device {AUDIO:?}: why must not be empty")
    );
    // An empty part names nothing Device Manager shows.
    assert_eq!(
        err(r"PCI\\X", "a", "b"),
        r#"device "PCI\\\\X": give the whole device instance path: <bus>\<device>\<instance>"#
    );
    assert_eq!(
        err(&format!("{AUDIO}\\"), "a", "b"),
        format!(
            "device {:?}: give the whole device instance path: <bus>\\<device>\\<instance>",
            format!("{AUDIO}\\")
        )
    );
}

#[test]
fn an_instance_path_holds_only_pnp_s_own_characters() {
    // dasio#181: printable ASCII, no space, no comma, as Windows makes
    // them; the guard's case-blind compare is then exact.
    let err = |id: &str| {
        parse(&format!(
            "name = \"dsg\"\n[[device]]\ninstance_id = \"{id}\"\nname = \"a\"\nwhy = \"b\"\n"
        ))
        .unwrap_err()
    };
    let charset = |id: &str| {
        format!(
            "device {id:?}: give the device instance path as Device Manager shows it: printable ASCII, no spaces or commas"
        )
    };
    for id in [
        r"PCI\VEN_8086 &DEV_54C8\3&1",
        r"USB\VID_1,PID_2\3&1",
        "PCI\\VEN_8086\\3&1\t2",
        r"PCI\VEN_8086&DEV_54C8\3&1é",
        r"PCI\VEN_8086&DEV_54C8\3&1İ",
    ] {
        let toml_id = id.replace('\\', r"\\");
        assert_eq!(err(&toml_id), charset(id), "{id:?}");
    }
    // Real paths with braces, dots, `#` and `&` are fine.
    for id in [
        r"SWD\MMDEVAPI\{0.0.0.00000000}.{A1B2C3D4-0000-4000-8000-00000000000A}",
        r"STORAGE\VOLUME\{6B1D8E5A-1C4F-11EE-9B7E-806E6F6E6963}#0000000000100000",
        r"HDAUDIO\FUNC_01&VEN_10EC&DEV_0257&SUBSYS_17AA22D2&REV_1000\4&D0DF0A3&0&0001",
    ] {
        assert_eq!(profile(&entry(id, "x")).device[0].instance_id, id);
    }
}

#[test]
fn a_device_named_twice_is_refused_whatever_its_case() {
    let e = parse(&format!(
        "name = \"dsg\"\n{}{}",
        entry(AUDIO, "a"),
        entry(&AUDIO.to_ascii_lowercase(), "b")
    ))
    .unwrap_err();
    assert_eq!(
        e,
        format!("device {:?} is named twice", AUDIO.to_ascii_lowercase())
    );
    // Two different devices are fine.
    assert_eq!(
        profile(&format!("{}{}", entry(AUDIO, "a"), entry(BT, "b")))
            .device
            .len(),
        2
    );
}

#[test]
fn an_unknown_field_is_refused() {
    let e = parse(&format!(
        "name = \"dsg\"\n{}state = \"on\"\n",
        entry(AUDIO, "a")
    ))
    .unwrap_err();
    assert!(e.contains("unknown field `state`"), "{e}");
}

#[test]
fn off_needs_the_live_disable_and_the_boot_flag_or_no_connection() {
    assert!(state(true, Some(22), true).off());
    assert!(state(false, None, true).off());
    assert!(!state(true, Some(22), false).off());
    assert!(!state(true, None, true).off());
    assert!(!state(true, Some(10), true).off());
    assert!(!state(true, None, false).off());
    assert!(!state(false, None, false).off());
}

#[test]
fn each_state_that_is_not_off_reads_as_what_it_is() {
    assert_eq!(state(true, None, false).describe(), "on");
    assert_eq!(state(true, Some(10), false).describe(), "on (problem 10)");
    assert_eq!(
        state(true, None, true).describe(),
        "on, off only from the next boot"
    );
    assert_eq!(
        state(true, Some(28), true).describe(),
        "on (problem 28), off only from the next boot"
    );
    assert_eq!(
        state(true, Some(22), false).describe(),
        "off until the next boot (ConfigFlags does not hold it)"
    );
    assert_eq!(
        state(false, None, false).describe(),
        "not connected, on when connected"
    );
    assert_eq!(state(false, None, true).describe(), "off");
}

#[test]
fn a_device_that_is_off_is_no_problem_and_nothing_to_apply() {
    let p = profile(&format!("{}{}", entry(AUDIO, "audio"), entry(BT, "bt")));
    let r = pc(&[(AUDIO, OFF), (BT, state(false, None, true))]);
    assert_eq!(check(&p, &r), Vec::<Problem>::new());
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn a_device_that_is_on_is_a_warn_and_apply_disables_it() {
    let p = profile(&entry(AUDIO, "onboard audio"));
    let r = pc(&[(AUDIO, state(true, None, false))]);
    assert_eq!(
        check(&p, &r),
        vec![Problem {
            what: format!("device onboard audio ({AUDIO})"),
            found: "on".into(),
            want: "off".into(),
            why: "not needed (onboard audio)".into(),
            must: false,
        }]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_off_only_until_the_boot_or_unplugged_and_on_is_planned_too() {
    let p = profile(&format!("{}{}", entry(AUDIO, "audio"), entry(BT, "bt")));
    let r = pc(&[
        (AUDIO, state(true, Some(22), false)),
        (BT, state(false, None, false)),
    ]);
    let found: Vec<String> = check(&p, &r).into_iter().map(|pr| pr.found).collect();
    assert_eq!(
        found,
        [
            "off until the next boot (ConfigFlags does not hold it)",
            "not connected, on when connected"
        ]
    );
    assert_eq!(
        plan(&p, &r),
        vec![Action::Disable(AUDIO.into()), Action::Disable(BT.into())]
    );
}

#[test]
fn the_instance_path_is_matched_without_case() {
    let p = profile(&entry(&AUDIO.to_ascii_lowercase(), "audio"));
    assert_eq!(check(&p, &pc(&[(AUDIO, OFF)])), Vec::<Problem>::new());
}

#[test]
fn a_missing_device_is_a_problem_unless_absent_ok_and_never_planned() {
    let missing = profile(&entry(BT, "bt"));
    let r = pc(&[]);
    assert_eq!(
        check(&missing, &r),
        vec![Problem {
            what: format!("device bt ({BT})"),
            found: "not on this PC".into(),
            want: "off".into(),
            why: "not needed (bt)".into(),
            must: false,
        }]
    );
    assert_eq!(plan(&missing, &r), Vec::<Action>::new());
    let ok = profile(&format!("{}absent_ok = true\n", entry(BT, "bt")));
    assert_eq!(check(&ok, &r), Vec::<Problem>::new());
    assert_eq!(plan(&ok, &r), Vec::<Action>::new());
}

#[test]
fn an_unreadable_device_is_a_problem_and_apply_tries_it() {
    let p = profile(&entry(AUDIO, "audio"));
    let mut r = pc(&[]);
    r.dev.device_err = Some("CM_Locate_DevNode: CONFIGRET 51".into());
    assert_eq!(
        check(&p, &r),
        vec![Problem {
            what: format!("device audio ({AUDIO})"),
            found: "unreadable (CM_Locate_DevNode: CONFIGRET 51)".into(),
            want: "off".into(),
            why: "not needed (audio)".into(),
            must: false,
        }]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn the_dante_nic_is_never_disabled_and_its_entry_is_a_problem() {
    // mbc has no other NIC: disabling it cuts the PC off.
    let p = profile(&format!(
        "{}{}",
        entry(&NIC.to_ascii_lowercase(), "dante nic"),
        entry(AUDIO, "audio")
    ));
    let r = pc(&[
        (NIC, state(true, None, false)),
        (AUDIO, state(true, None, false)),
    ]);
    let problems = check(&p, &r);
    assert_eq!(
        problems[0],
        Problem {
            what: format!("device dante nic ({})", NIC.to_ascii_lowercase()),
            found: "the Dante NIC".into(),
            want: "an entry for another device".into(),
            why: "never disabled by the profile: it cuts the PC off (dasio#181)".into(),
            must: false,
        }
    );
    assert_eq!(problems.len(), 2);
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_the_dante_nic_hangs_on_is_never_disabled() {
    // Its hub or host controller off cuts the USB NIC off just the same.
    let p = profile(&format!(
        "{}{}{}",
        entry(HUB, "usb hub"),
        entry(&XHCI.to_ascii_lowercase(), "usb host"),
        entry(AUDIO, "audio")
    ));
    let on = state(true, None, false);
    let r = pc(&[(HUB, on), (XHCI, on), (AUDIO, on)]);
    let found: Vec<(String, String)> = check(&p, &r)
        .into_iter()
        .map(|pr| (pr.found, pr.want))
        .collect();
    let hangs = (
        "a device the Dante NIC hangs on".to_string(),
        "an entry for another device".to_string(),
    );
    assert_eq!(found, [hangs.clone(), hangs, ("on".into(), "off".into())]);
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn without_the_dante_nic_s_path_nothing_is_disabled() {
    // Fail closed: an unread NIC must not turn the guard off.
    let p = profile(&format!(
        "{}{}",
        entry(NIC, "dante nic"),
        entry(AUDIO, "audio")
    ));
    let mut r = pc(&[
        (NIC, state(true, None, false)),
        (AUDIO, state(true, None, false)),
    ]);
    r.dev.nic_chain = Vec::new();
    assert_eq!(
        check(&p, &r),
        vec![Problem {
            what: "the 2 [[device]] entries".into(),
            found: "not checked: the Dante NIC's device path was not read".into(),
            want: "the Dante NIC's device and its parents, so no entry can cut it off".into(),
            why: "no device is disabled without them (dasio#181)".into(),
            must: false,
        }]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
    // A profile without devices needs no NIC path.
    assert_eq!(check(&profile(""), &r), Vec::<Problem>::new());
}

#[test]
fn a_disable_says_what_it_sets_and_needs_nothing_more() {
    let a = Action::Disable(AUDIO.into());
    assert_eq!(describe(&a), format!("device {AUDIO} off"));
    assert_eq!(
        takes_effect(&a),
        "nothing more (the device stops now; ConfigFlags keeps it off at every boot)"
    );
}

#[test]
fn the_site_s_device_entry_wins_over_the_baseline_s_and_the_rest_is_kept() {
    let base = parse_with(
        &format!(
            "name = \"base\"\n{}{}",
            entry(AUDIO, "base audio"),
            entry(BT, "base bt")
        ),
        false,
    )
    .unwrap();
    let site = profile(&format!(
        "{}{}",
        entry(&AUDIO.to_ascii_lowercase(), "site audio"),
        entry(NIC, "site nic")
    ));
    let names: Vec<String> = merge(&base, &site)
        .device
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, ["base bt", "site audio", "site nic"]);
    assert_eq!(
        baseline_only(&base)
            .device
            .into_iter()
            .map(|d| d.name)
            .collect::<Vec<_>>(),
        ["base audio", "base bt"]
    );
}

#[test]
fn the_bridge_s_in_process_check_keeps_the_devices() {
    let p = profile(&entry(AUDIO, "audio"));
    assert_eq!(in_process(&p).device, p.device);
}
