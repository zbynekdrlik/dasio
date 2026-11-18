//! `[[device]]` entries (dasio#181): a device the PC does not need, held
//! off; never the Dante NIC.

use super::*;

const AUDIO: &str = r"PCI\VEN_8086&DEV_54C8&SUBSYS_17951025&REV_00\3&11583659&0&FB";
const BT: &str = r"USB\VID_8087&PID_0026\5&C99FFEC&0&10";
const NIC: &str = r"USB\VID_0BDA&PID_8156\4F13000001";
/// The hub and the host controller mbc's USB Dante NIC hangs on.
const HUB: &str = r"USB\ROOT_HUB30\5&1EF46A2D&0&0";
const XHCI: &str = r"PCI\VEN_1022&DEV_1639&SUBSYS_387017AA&REV_00\4&2D99F355&0&0441";

fn profile(entries: &str) -> Profile {
    parse(&format!("name = \"dsg\"\n{entries}")).unwrap()
}

fn entry(id: &str, name: &str) -> String {
    format!("[[device]]\ninstance_id = '{id}'\nname = \"{name}\"\nwhy = \"not needed ({name})\"\n")
}

fn state(present: bool, problem: Option<u32>, config_disabled: bool) -> DevState {
    DevState {
        present,
        problem,
        config_disabled,
    }
}

fn pc(devices: &[(&str, DevState)]) -> Pc {
    Pc {
        devices: devices
            .iter()
            .map(|(id, s)| (id.to_ascii_uppercase(), *s))
            .collect(),
        nic_chain: vec![NIC.into(), HUB.into(), XHCI.into(), r"HTREE\ROOT\0".into()],
        ..Pc::default()
    }
}

const OFF: DevState = DevState {
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
fn an_unreadable_device_is_a_problem_and_never_disabled() {
    // Not read, its parents are not known either: it may hang below a
    // network path, so apply leaves it.
    let p = profile(&entry(AUDIO, "audio"));
    let r = Pc {
        device_err: Some("CM_Locate_DevNode: CONFIGRET 51".into()),
        ..pc(&[])
    };
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
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
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
    let r = Pc {
        nic_chain: Vec::new(),
        ..pc(&[
            (NIC, state(true, None, false)),
            (AUDIO, state(true, None, false)),
        ])
    };
    assert_eq!(
        check(&p, &r),
        vec![Problem {
            what: "the 2 [[device]] entries".into(),
            found: "not checked: the Dante NIC's device path was not read".into(),
            want: "the Dante NIC's device, every adapter with an address and their parents, so no entry can cut the PC off".into(),
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

/// dsg's LAN VLAN adapter (layout B): the remote path, with an address.
const LAN: &str = r"ROOT\NET\0001";
const LAN_PARENT: &str = r"ROOT\RTVNIC\0000";

fn with_lan(devices: &[(&str, DevState)]) -> Pc {
    Pc {
        addressed: vec![
            vec![NIC.into(), HUB.into(), XHCI.into(), r"HTREE\ROOT\0".into()],
            vec![LAN.into(), LAN_PARENT.into(), r"HTREE\ROOT\0".into()],
        ],
        ..pc(devices)
    }
}

#[test]
fn an_adapter_with_an_address_and_what_it_hangs_on_are_never_disabled() {
    // dsg's LAN89 carries the remote path: off, the PC is out of reach.
    let p = profile(&format!(
        "{}{}{}",
        entry(&LAN.to_ascii_lowercase(), "lan"),
        entry(LAN_PARENT, "lan parent"),
        entry(AUDIO, "audio")
    ));
    let on = state(true, None, false);
    let r = with_lan(&[(LAN, on), (LAN_PARENT, on), (AUDIO, on)]);
    let found: Vec<String> = check(&p, &r).into_iter().map(|pr| pr.found).collect();
    assert_eq!(
        found,
        [
            "a network adapter with an address (a path to the PC)",
            "a device a network adapter with an address hangs on",
            "on"
        ]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_below_the_dante_nic_or_an_addressed_adapter_is_never_disabled() {
    const UNDER_NIC: &str = r"USB\VID_0BDA&PID_8156&MI_01\6&1&0&0001";
    const UNDER_LAN: &str = r"SWD\NETBRIDGE\0000";
    let p = profile(&format!(
        "{}{}{}",
        entry(UNDER_NIC, "nic function"),
        entry(UNDER_LAN, "lan child"),
        entry(AUDIO, "audio")
    ));
    let on = state(true, None, false);
    let mut r = with_lan(&[(UNDER_NIC, on), (UNDER_LAN, on), (AUDIO, on)]);
    r.parents = [
        (UNDER_NIC, vec![NIC.to_ascii_lowercase(), HUB.into()]),
        (UNDER_LAN, vec![LAN.into(), LAN_PARENT.into()]),
        // Hanging on the same hub as the NIC is not hanging below it.
        (AUDIO, vec![HUB.into(), XHCI.into()]),
    ]
    .into_iter()
    .map(|(id, p)| (id.to_ascii_uppercase(), p))
    .collect();
    let below = "a device below the Dante NIC or a network adapter with an address";
    let problems = check(&p, &r);
    assert_eq!(
        problems
            .iter()
            .map(|pr| pr.found.as_str())
            .collect::<Vec<_>>(),
        [below, below, "on"]
    );
    assert_eq!(problems[0].want, "an entry for another device");
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_whose_parents_are_unreadable_is_never_disabled() {
    let p = profile(&entry(AUDIO, "audio"));
    let r = Pc {
        parents_err: Some("CM_Get_Parent: CONFIGRET 13".into()),
        ..pc(&[(AUDIO, state(true, None, false))])
    };
    assert_eq!(
        check(&p, &r)[0].found,
        "unreadable (its parents: CM_Get_Parent: CONFIGRET 13)"
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
    // A device that is off needs no parents.
    let off = Pc {
        parents_err: Some("x".into()),
        ..pc(&[(AUDIO, OFF)])
    };
    assert_eq!(check(&p, &off), Vec::<Problem>::new());
}

#[test]
fn without_the_addressed_adapters_nothing_is_disabled() {
    let p = profile(&format!("{}{}", entry(AUDIO, "audio"), entry(BT, "bt")));
    let r = Pc {
        addressed_err: Some("GetAdaptersAddresses: os error 8".into()),
        ..pc(&[(AUDIO, state(true, None, false))])
    };
    assert_eq!(
        check(&p, &r),
        vec![Problem {
            what: "the 2 [[device]] entries".into(),
            found: "not checked: the PC's network adapters were not read (GetAdaptersAddresses: os error 8)".into(),
            want: "the Dante NIC's device, every adapter with an address and their parents, so no entry can cut the PC off".into(),
            why: "no device is disabled without them (dasio#181)".into(),
            must: false,
        }]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}
