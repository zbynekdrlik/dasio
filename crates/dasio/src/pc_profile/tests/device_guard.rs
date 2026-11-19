//! The `[[device]]` guard beyond the Dante NIC (dasio#181): an entry that
//! would be disabled is refused when disabling it can cut any network
//! path, whatever the adapter's address or link; it fails closed.

use std::cell::Cell;
use std::collections::HashMap;

use super::device_kind::{AUDIO, BT, HUB, NIC, OFF, ROOT, entry, pc, profile, state};
use super::*;

/// The remote path: a second NIC with no Dante address (dsg's LAN, layout
/// B), the PCIe root port it hangs on and the PCI root bus above that.
const LAN: &str = r"PCI\VEN_10EC&DEV_8125&SUBSYS_81251849&REV_05\4&2F1C9A3&0&00E4";
const PORT: &str = r"PCI\VEN_8086&DEV_7AB8&SUBSYS_7AB81849&REV_11\3&11583659&0&E4";
const PCI_ROOT: &str = r"ACPI\PNP0A08\0";
/// A VF with no address under its virtual PCI bus (the runner's Azure
/// adapter, PR #183's CI).
const VF: &str = r"PCI\VEN_15B3&DEV_101A&SUBSYS_019015B3&REV_80\5&7A31B2E&0&0";
const VPCI: &str =
    r"VMBUS\{44C4F61D-4444-4400-9D52-802E27EDE19F}\{9A1B2C3D-0000-4000-8000-00000000000F}";
/// A device hanging below the LAN adapter, and one below the Dante NIC.
const BELOW_LAN: &str = r"SWD\LANPTP\{0F3B6A10-1D2C-4E5F-8A9B-0C1D2E3F4A5B}";
const BELOW_NIC: &str = r"USB\VID_0BDA&PID_8156&MI_01\6&3A1B2C3D&0&0001";
/// Devices tied to the others only by removal relations.
const HOST: &str = r"ROOT\VMBUSHOST\0000";
const REL: &str = r"ROOT\VMBUSREL\0000";
const OTHER: &str = r"ROOT\VFOWNER\0000";
/// A removal-relation cycle that reaches no adapter, and its leaf.
const CYC_A: &str = r"ROOT\CYCLEA\0000";
const CYC_B: &str = r"ROOT\CYCLEB\0000";
const LEAF: &str = r"ROOT\LEAF\0000";

const ON: DevState = DevState {
    present: true,
    problem: None,
    config_disabled: false,
};

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// mbc-like: the Dante NIC, plus the LAN adapter and the VF, every
/// device in `devices` with its state.
fn guarded_pc(devices: &[(&str, DevState)]) -> Pc {
    let mut r = pc(devices);
    r.dev.adapters.push(Adapter {
        id: LAN.into(),
        parents: ids(&[PORT, PCI_ROOT, ROOT]),
    });
    r.dev.adapters.push(Adapter {
        id: VF.into(),
        parents: ids(&[VPCI, ROOT]),
    });
    r
}

/// `id`'s parents or what stops with it (`table`), as the reader gives
/// them.
fn link(table: &mut HashMap<String, Vec<String>>, id: &str, to: &[&str]) {
    table.insert(id.to_ascii_uppercase(), ids(to));
}

/// Each problem's found and want.
fn found(p: &Profile, r: &Pc) -> Vec<(String, String)> {
    check(p, r).into_iter().map(|x| (x.found, x.want)).collect()
}

fn refused(it: &str) -> (String, String) {
    (it.to_string(), "an entry for another device".to_string())
}

fn wants_off(it: &str) -> (String, String) {
    (it.to_string(), "off".to_string())
}

#[test]
fn every_network_adapter_is_refused_whatever_its_address_or_link() {
    // The LAN adapter carries no Dante address, the VF no address at all
    // and no link: both are network paths.
    let p = profile(&format!(
        "{}{}{}",
        entry(LAN, "lan"),
        entry(&VF.to_ascii_lowercase(), "vf"),
        entry(AUDIO, "audio")
    ));
    let r = guarded_pc(&[(LAN, ON), (VF, state(true, Some(10), false)), (AUDIO, ON)]);
    assert_eq!(
        check(&p, &r)[0],
        Problem {
            what: format!("device lan ({LAN})"),
            found: "a network adapter".into(),
            want: "an entry for another device".into(),
            why: "never disabled by the profile: it cuts the PC off (dasio#181)".into(),
            must: false,
        }
    );
    assert_eq!(
        found(&p, &r),
        [
            refused("a network adapter"),
            refused("a network adapter"),
            wants_off("on")
        ]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_a_network_adapter_hangs_on_is_refused() {
    let p = profile(&format!(
        "{}{}{}",
        entry(PORT, "root port"),
        entry(PCI_ROOT, "pci root"),
        entry(&VPCI.to_ascii_lowercase(), "vpci")
    ));
    let r = guarded_pc(&[(PORT, ON), (PCI_ROOT, ON), (VPCI, ON)]);
    assert_eq!(
        found(&p, &r),
        [
            refused(&format!("a device the network adapter {LAN} hangs on")),
            refused(&format!("a device the network adapter {LAN} hangs on")),
            refused(&format!("a device the network adapter {VF} hangs on")),
        ]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn a_device_below_a_network_adapter_is_refused() {
    let p = profile(&format!(
        "{}{}{}",
        entry(BELOW_LAN, "lan ptp"),
        entry(BELOW_NIC, "nic function"),
        entry(AUDIO, "audio")
    ));
    let mut r = guarded_pc(&[(BELOW_LAN, ON), (BELOW_NIC, ON), (AUDIO, ON)]);
    let lan = LAN.to_ascii_lowercase();
    let parents = &mut r.dev.parents;
    link(parents, BELOW_LAN, &[&lan, PORT, PCI_ROOT, ROOT]);
    link(parents, BELOW_NIC, &[NIC, HUB, ROOT]);
    link(parents, AUDIO, &[PCI_ROOT, ROOT]);
    assert_eq!(
        found(&p, &r),
        [
            refused(&format!("a device below the network adapter {LAN}")),
            refused(&format!("a device below the network adapter {NIC}")),
            wants_off("on"),
        ]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(AUDIO.into())]);
}

#[test]
fn a_device_whose_removal_stops_a_network_adapter_is_refused() {
    // Disabling a device removes its subtree and its removal relations,
    // and theirs (as Windows does): HOST takes REL, REL takes the VF's
    // bus; OTHER takes the VF itself; the cycle reaches no adapter.
    let p = profile(&format!(
        "{}{}{}",
        entry(HOST, "host"),
        entry(OTHER, "other"),
        entry(CYC_A, "cycle")
    ));
    let mut r = guarded_pc(&[(HOST, ON), (OTHER, ON), (CYC_A, ON)]);
    let vpci = VPCI.to_ascii_lowercase();
    let cyc_b = CYC_B.to_ascii_lowercase();
    let removes = &mut r.dev.removes;
    link(removes, HOST, &[LEAF, REL]);
    link(removes, REL, &[HOST, &vpci]);
    link(removes, OTHER, &[LEAF, VF]);
    link(removes, CYC_A, &[CYC_B]);
    link(removes, CYC_B, &[CYC_A, LEAF, &cyc_b]);
    assert_eq!(
        found(&p, &r),
        [
            refused(&format!(
                "a device whose removal stops the network adapter {VF} ({vpci} stops with it)"
            )),
            refused(&format!(
                "a device whose removal stops the network adapter {VF} ({VF} stops with it)"
            )),
            wants_off("on"),
        ]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(CYC_A.into())]);
}

#[test]
fn a_removal_two_steps_down_stops_an_adapter_s_parent_too() {
    // REL is reached only through HOST, and it takes the LAN's root port.
    let p = profile(&entry(HOST, "host"));
    let mut r = guarded_pc(&[(HOST, ON)]);
    link(&mut r.dev.removes, HOST, &[REL]);
    link(&mut r.dev.removes, REL, &[LEAF, PORT]);
    assert_eq!(
        found(&p, &r),
        [refused(&format!(
            "a device whose removal stops the network adapter {LAN} ({PORT} stops with it)"
        ))]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn the_adapters_are_read_once_and_only_for_an_entry_that_would_be_disabled() {
    // dsg's Wi-Fi and KDNIC are network adapters already off: fine, never
    // judged, and an unreadable adapter list does not matter then.
    let off_only = profile(&format!(
        "{}{}absent_ok = true\n",
        entry(LAN, "wifi"),
        entry(BT, "bt")
    ));
    let mut r = guarded_pc(&[(LAN, OFF)]);
    r.dev.net_err = Some("CM_Get_Device_ID_List: CONFIGRET 13".into());
    assert_eq!(check(&off_only, &r), Vec::<Problem>::new());
    assert_eq!(plan(&off_only, &r), Vec::<Action>::new());
    assert_eq!(r.dev.net_reads.get(), 0);
    // Two entries that would be disabled: one read for the check, and
    // nothing disabled while it fails (fail closed).
    let on = profile(&format!("{}{}", entry(AUDIO, "audio"), entry(BT, "bt")));
    let mut r = guarded_pc(&[(AUDIO, ON), (BT, state(false, None, false))]);
    r.dev.net_err = Some("CM_Get_Device_ID_List: CONFIGRET 13".into());
    assert_eq!(
        check(&on, &r),
        vec![
            Problem {
                what: format!("device audio ({AUDIO})"),
                found: "on; not disabled: the network adapters were not read (CM_Get_Device_ID_List: CONFIGRET 13)".into(),
                want: "off".into(),
                why: "not needed (audio)".into(),
                must: false,
            },
            Problem {
                what: format!("device bt ({BT})"),
                found: "not connected, on when connected; not disabled: the network adapters were not read (CM_Get_Device_ID_List: CONFIGRET 13)".into(),
                want: "off".into(),
                why: "not needed (bt)".into(),
                must: false,
            },
        ]
    );
    assert_eq!(r.dev.net_reads.get(), 1);
    assert_eq!(plan(&on, &r), Vec::<Action>::new());
    assert_eq!(r.dev.net_reads.get(), 2);
}

#[test]
fn an_entry_whose_relations_are_unread_is_never_disabled() {
    let p = profile(&format!("{}{}", entry(AUDIO, "audio"), entry(BT, "bt")));
    let mut r = guarded_pc(&[(AUDIO, ON), (BT, ON)]);
    link(&mut r.dev.removes, BT, &[CYC_A]);
    link(&mut r.dev.removes, CYC_A, &[LEAF]);
    r.dev.unread = ids(&[AUDIO, LEAF]);
    assert_eq!(
        found(&p, &r),
        [
            wants_off(&format!(
                "on; not disabled: its parents were not read (parents {AUDIO}: CONFIGRET 13)"
            )),
            wants_off(&format!(
                "on; not disabled: what stops with {LEAF} was not read (removes {LEAF}: CONFIGRET 13)"
            )),
        ]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn a_device_whose_state_is_unread_goes_through_the_guard() {
    // Its state unread, it may well be connected: the adapters, its
    // parents and what stops with it are all read before a disable.
    let p = profile(&format!("{}{}", entry(LAN, "lan"), entry(AUDIO, "audio")));
    let mut r = guarded_pc(&[]);
    r.dev.device_err = Some("CM_Locate_DevNode: CONFIGRET 51".into());
    r.dev.unread = ids(&[AUDIO]);
    assert_eq!(
        found(&p, &r),
        [
            refused("a network adapter"),
            wants_off(&format!(
                "unreadable (CM_Locate_DevNode: CONFIGRET 51); not disabled: its parents were not read (parents {AUDIO}: CONFIGRET 13)"
            )),
        ]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn a_device_not_connected_is_judged_by_its_path_alone() {
    // Nothing hangs on an unplugged device and its disable stops nothing
    // now: its relations are not read (they may not be readable).
    let p = profile(&entry(BT, "bt"));
    let mut r = guarded_pc(&[(BT, state(false, None, false))]);
    r.dev.unread = ids(&[BT]);
    assert_eq!(
        found(&p, &r),
        [wants_off("not connected, on when connected")]
    );
    assert_eq!(plan(&p, &r), vec![Action::Disable(BT.into())]);
}

#[test]
fn an_endless_tree_below_an_entry_counts_as_unread() {
    let p = profile(&entry(AUDIO, "audio"));
    let mut r = guarded_pc(&[(AUDIO, ON)]);
    r.dev.endless = Some(Cell::new(0));
    assert_eq!(
        found(&p, &r),
        [wants_off(
            "on; not disabled: more than 4096 devices stop with it"
        )]
    );
    // Exactly the bound was followed.
    assert_eq!(r.dev.endless.as_ref().map(Cell::get), Some(4096));
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}

#[test]
fn a_removal_that_stops_the_dante_nic_s_path_is_refused() {
    // Review of PR #196: the removal walk checks the Dante NIC's chain too,
    // not only the adapter list (the NIC may be missing from that list).
    let p = profile(&entry(OTHER, "other"));
    let mut r = guarded_pc(&[(OTHER, ON)]);
    // The NIC itself missing from the adapter list (a failed or partial
    // Net-class read): only its chain still names it.
    r.dev.adapters.retain(|a| !a.id.eq_ignore_ascii_case(NIC));
    link(&mut r.dev.removes, OTHER, &[HUB]);
    assert_eq!(
        found(&p, &r),
        [refused(&format!(
            "a device whose removal stops the Dante NIC ({HUB} stops with it)"
        ))]
    );
    assert_eq!(plan(&p, &r), Vec::<Action>::new());
}
