//! Several dasio instances on one PC never share CPUs (dasio#199, owner
//! 19.11.2026): the check's share problems and install.ps1's instance
//! step, on FOH's, mbc's and dsg's layouts.

use super::*;
use crate::pc_profile::layout::{SHARE_WHY, cpus, instance_exe, instance_room, shares};

/// `text` as a site profile, resolved for a PC of `cpus` CPUs.
fn on(cpus: usize, text: &str) -> Profile {
    let mut p = parse(&format!("name = \"pc\"\n{text}")).unwrap();
    resolve(&mut p, cpus).unwrap();
    p
}

fn entry(exe: &str, affinity: &str) -> String {
    format!("[[process]]\nexe = \"{exe}\"\naffinity = \"{affinity}\"\n")
}

const BAND: &str = "dasio-band-asio-dante.exe";
const MASTER: &str = "dasio-master-asio-dante.exe";

/// FOH as it runs (one instance): every pinned entry of its site profile,
/// 19.11.2026 (dasio#199's profile step), and `extra`.
fn foh(extra: &str) -> String {
    [
        entry("Ableton Live 12 Suite.exe", "0xFFFF0000"),
        entry("AbletonLiveMaster.exe", "0xFF00"),
        entry("AudioGridderServer.exe", "0xFF00"),
        entry(BAND, "0xF0"),
        entry("dsync.exe", "0xF"),
        entry("cg.exe", "0xF"),
        entry("dvs_service.exe", "0xF"),
        entry("crashpad_handler.exe", "0xFFFF0000"),
        entry("Max Helper.exe", "0xFFFF0000"),
        entry("AudioGridderPluginTray.exe", "0xFFFF0000"),
        entry("dsync-tray.exe", "0xF"),
        entry("TouchOSC Bridge.exe", "0xF"),
        entry("AbleSet.exe", "0xF"),
        entry("AMD Ryzen Master.exe", "{system}"),
        extra.to_string(),
    ]
    .concat()
}

/// Each problem as its one line.
fn lines(p: &Profile) -> Vec<String> {
    shares(p).iter().map(ToString::to_string).collect()
}

/// Each problem's entry and finding.
fn found(p: &Profile) -> Vec<(String, String)> {
    shares(p).into_iter().map(|x| (x.what, x.found)).collect()
}

fn pair(what: &str, found: &str) -> (String, String) {
    (what.to_string(), found.to_string())
}

#[test]
fn cpus_are_shown_as_runs() {
    assert_eq!(cpus(0), "");
    assert_eq!(cpus(0x1), "0");
    assert_eq!(cpus(0x2), "1");
    assert_eq!(cpus(0xF0), "4-7");
    assert_eq!(cpus(0xF000F), "0-3, 16-19");
    assert_eq!(cpus(0x5), "0, 2");
    assert_eq!(cpus(1u64 << 63), "63");
    assert_eq!(cpus(u64::MAX), "0-63");
    assert_eq!(cpus(0x8000_0000_0000_0001), "0, 63");
    assert_eq!(cpus(0xC000_0000_0000_0000), "62-63");
}

#[test]
fn an_instance_exe_is_dasio_s_but_neither_the_tools_nor_the_keeper() {
    assert!(instance_exe(BAND));
    assert!(instance_exe("DASIO-DSG.EXE"));
    assert!(!instance_exe("dasio.exe"));
    assert!(!instance_exe("Dasio.exe"));
    assert!(!instance_exe("dasio-keep.exe"));
    assert!(!instance_exe("DASIO-KEEP.EXE"));
    assert!(!instance_exe("dsync.exe"));
}

#[test]
fn foh_as_it_runs_has_no_share() {
    assert_eq!(lines(&on(32, &foh(""))), Vec::<String>::new());
}

#[test]
fn a_process_on_every_cpu_shares_the_instance_s_cpus() {
    // AMD Ryzen Master's Lasso rule was 0-31: band dasio's 4-7 too.
    let p = on(32, &foh(&entry("Tool.exe", "0xFFFFFFFF")));
    assert_eq!(
        lines(&p),
        vec![format!(
            "process Tool.exe affinity: CPUs 0-31 share 4-7 with dasio:1 (the profile wants none of dasio's CPUs (dasio:1 4-7, {BAND} 4-7)) — {SHARE_WHY}"
        )]
    );
}

#[test]
fn a_second_instance_s_cpus_are_master_live_s_today() {
    // instances = 2 on FOH: dasio:2 is 8-11, where master Live and
    // AudioGridderServer run.
    let p = on(32, &format!("instances = 2\n{}", foh("")));
    assert_eq!(
        found(&p),
        vec![
            pair(
                "process AbletonLiveMaster.exe affinity",
                "CPUs 8-15 share 8-11 with dasio:2"
            ),
            pair(
                "process AudioGridderServer.exe affinity",
                "CPUs 8-15 share 8-11 with dasio:2"
            ),
        ]
    );
    assert_eq!(
        shares(&p)[0].want,
        format!("none of dasio's CPUs (dasio:1 4-7, dasio:2 8-11, {BAND} 4-7)")
    );
}

#[test]
fn an_instance_entry_s_own_mask_counts_without_the_layout_s_instance() {
    // The Lasso-era master dasio entry on 8-11 with instances = 1: master
    // Live and AudioGridderServer share its CPUs.
    let p = on(32, &foh(&entry(MASTER, "0xF00")));
    assert_eq!(
        found(&p),
        vec![
            pair(
                "process AbletonLiveMaster.exe affinity",
                &format!("CPUs 8-15 share 8-11 with {MASTER}")
            ),
            pair(
                "process AudioGridderServer.exe affinity",
                &format!("CPUs 8-15 share 8-11 with {MASTER}")
            ),
        ]
    );
}

#[test]
fn each_shared_cpu_is_named_once_by_its_first_owner() {
    // Band's entry on dasio:1's own CPUs adds nothing to the line; the part
    // of a wider instance entry outside the layout's instance is named by
    // the entry.
    let p = on(
        32,
        &[entry(BAND, "0x1F0"), entry("Tool.exe", "0x3F0")].concat(),
    );
    assert_eq!(
        found(&p),
        vec![pair(
            "process Tool.exe affinity",
            &format!("CPUs 4-9 share 4-7 with dasio:1, 8 with {BAND}")
        )]
    );
}

#[test]
fn two_instance_entries_on_a_common_cpu_share_it() {
    let p = on(32, &[entry(BAND, "0xF0"), entry(MASTER, "0x3C0")].concat());
    assert_eq!(
        found(&p),
        vec![pair(
            &format!("process {BAND} affinity"),
            &format!("CPUs 4-7 share 6-7 with {MASTER}")
        )]
    );
    // Apart, they are fine.
    let apart = on(32, &[entry(BAND, "0xF0"), entry(MASTER, "0xF00")].concat());
    assert!(shares(&apart).is_empty(), "{:?}", lines(&apart));
}

#[test]
fn an_instance_entry_on_two_instances_cpus_is_a_share() {
    // Band on 4-11 with instances = 2, before any entry for the second
    // instance's exe.
    let p = on(32, &format!("instances = 2\n{}", entry(BAND, "0xFF0")));
    assert_eq!(
        found(&p),
        vec![pair(
            &format!("process {BAND} affinity"),
            "CPUs 4-11 span two instances: 4-7 of dasio:1, 8-11 of dasio:2"
        )]
    );
    // One instance and the DAW's CPUs is fine: dsg's bridge.
    let dsg = on(4, &entry("dasio-dsg.exe", "0xE"));
    assert!(shares(&dsg).is_empty(), "{:?}", lines(&dsg));
    // So with two instances: band's own 4-7 and the DAW's 12 touch one
    // instance's CPUs only.
    let reach = on(32, &format!("instances = 2\n{}", entry(BAND, "0x10F0")));
    assert!(shares(&reach).is_empty(), "{:?}", lines(&reach));
}

#[test]
fn the_keeper_and_the_tools_own_no_cpus_but_may_not_take_an_instance_s() {
    // The keeper and dasio.exe pinned to the system CPUs: dsync there
    // shares nothing with them.
    let p = on(
        32,
        &[
            entry("dasio-keep.exe", "0xF"),
            entry("dasio.exe", "0xF"),
            entry("dsync.exe", "0xF"),
        ]
        .concat(),
    );
    assert!(shares(&p).is_empty(), "{:?}", lines(&p));
    // On an instance's CPUs they are judged like any other entry.
    let on_band = on(32, &entry("dasio-keep.exe", "0xF0"));
    assert_eq!(
        found(&on_band),
        vec![pair(
            "process dasio-keep.exe affinity",
            "CPUs 4-7 share 4-7 with dasio:1"
        )]
    );
}

#[test]
fn others_affinity_on_an_instance_s_cpus_is_a_share() {
    // mbc: 12 CPUs, dasio:1 2-3.
    let ok = on(12, "others_affinity = \"{system}\"\n");
    assert!(shares(&ok).is_empty());
    let bad = on(12, "others_affinity = \"0xF\"\n");
    assert_eq!(
        found(&bad),
        vec![pair("others_affinity", "CPUs 0-3 share 2-3 with dasio:1")]
    );
}

#[test]
fn the_pcs_as_they_run_have_no_share() {
    // mbc and dsg (a bridge spans its own DAW CPU, 1-3).
    let mbc = on(
        12,
        &format!(
            "others_affinity = \"{{system}}\"\n{}{}{}",
            entry("dasio-mbc-snv.exe", "0xC"),
            entry("Ableton Live 12 Suite.exe", "0xFF0"),
            entry("dsync.exe", "0x3")
        ),
    );
    assert!(shares(&mbc).is_empty(), "{:?}", lines(&mbc));
    let dsg = on(
        4,
        &[
            entry("dasio-dsg.exe", "0xE"),
            entry("dsync.exe", "0x1"),
            entry("SGDawNodeService.exe", "0x1"),
        ]
        .concat(),
    );
    assert!(shares(&dsg).is_empty(), "{:?}", lines(&dsg));
}

#[test]
fn without_a_computed_layout_only_the_entries_count() {
    let mut p = parse(&format!(
        "name = \"pc\"\n{}",
        [
            entry("dasio-a.exe", "0xF0"),
            entry("Live.exe", "0x30"),
            entry("Other.exe", "0xF00"),
        ]
        .concat()
    ))
    .unwrap();
    assert_eq!(p.layout, None);
    assert_eq!(
        found(&p),
        vec![pair(
            "process Live.exe affinity",
            "CPUs 4-5 share 4-5 with dasio-a.exe"
        )]
    );
    assert_eq!(shares(&p)[0].want, "none of dasio's CPUs (dasio-a.exe 4-7)");
    // No dasio at all: nothing to share.
    p.process.retain(|e| e.exe != "dasio-a.exe");
    assert!(shares(&p).is_empty());
}

#[test]
fn a_share_is_a_problem_of_the_check_never_a_must_have() {
    let p = on(32, &foh(&entry("Tool.exe", "0xFFFFFFFF")));
    let found = check(&p, &Pc::default());
    let share: Vec<&Problem> = found
        .iter()
        .filter(|x| x.what == "process Tool.exe affinity")
        .collect();
    assert_eq!(share.len(), 1, "{found:?}");
    assert!(!share[0].must);
    assert_eq!(share[0].why, SHARE_WHY);
}

#[test]
fn resolve_keeps_the_layout_it_computed() {
    let p = on(32, "instances = 2\n");
    assert_eq!(p.layout, Some(Layout::of(32, 2)));
}

#[test]
fn an_instance_has_room_when_the_layout_holds_it_and_nobody_pins_its_cpus() {
    let p = on(32, &foh(""));
    assert_eq!(
        instance_room(&p, 1, BAND),
        Ok(format!(
            "instance 1 ({BAND}): CPUs 4-7 are its own ({})",
            Layout::of(32, 1).describe()
        ))
    );
    // The exe compares without case, as Windows names it.
    assert!(instance_room(&p, 1, "DASIO-BAND-ASIO-DANTE.EXE").is_ok());
}

#[test]
fn an_instance_beyond_the_layout_is_refused() {
    let p = on(32, &foh(""));
    assert_eq!(
        instance_room(&p, 2, MASTER),
        Err("instance 2 has no CPUs of its own: the site profile has instances = 1 (set instances = 2 there first; dasio pc-profile layout shows the CPUs it then gives)".into())
    );
    assert_eq!(
        instance_room(&p, 0, MASTER),
        Err("instance 0: instances are numbered from 1".into())
    );
    // A layout that names it but has no CPUs for it.
    let small = on(4, "instances = 2\n");
    assert_eq!(
        instance_room(&small, 2, MASTER),
        Err("instance 2 has no CPUs of its own: dasio:2 needs CPUs 3-4, the PC has 4".into())
    );
    // Not resolved: no layout to judge by.
    let raw = parse("name = \"pc\"\n").unwrap();
    assert_eq!(
        instance_room(&raw, 1, BAND),
        Err("the profile's CPU layout was not computed".into())
    );
}

#[test]
fn a_second_instance_on_foh_waits_until_master_live_moves() {
    let two = |master: &str| {
        on(
            32,
            &format!(
                "instances = 2\n{}",
                foh("").replace("affinity = \"0xFF00\"", &format!("affinity = \"{master}\""))
            ),
        )
    };
    assert_eq!(
        instance_room(&two("0xFF00"), 2, MASTER),
        Err("instance 2's CPUs 8-11 are pinned to AbletonLiveMaster.exe (8-15), AudioGridderServer.exe (8-15) too: move them off in the site profile first (several dasio on one PC must not fight, dasio#199)".into())
    );
    // Instance 1 is not touched by master's CPUs.
    assert!(instance_room(&two("0xFF00"), 1, BAND).is_ok());
    // Master Live on 12-15: room for instance 2, and no share left.
    let moved = two("0xF000");
    assert!(
        instance_room(&moved, 2, MASTER).is_ok(),
        "{:?}",
        instance_room(&moved, 2, MASTER)
    );
    assert!(shares(&moved).is_empty(), "{:?}", lines(&moved));
}

#[test]
fn another_instance_s_entry_on_its_cpus_refuses_it() {
    // Band spread over 4-11 before the second instance comes: instance 2
    // (master's exe) has no CPUs of its own; band itself keeps its room.
    let p = on(32, &format!("instances = 2\n{}", entry(BAND, "0xFF0")));
    assert_eq!(
        instance_room(&p, 2, MASTER),
        Err(format!(
            "instance 2's CPUs 8-11 are pinned to {BAND} (4-11) too: move them off in the site profile first (several dasio on one PC must not fight, dasio#199)"
        ))
    );
    assert!(instance_room(&p, 1, BAND).is_ok());
}

#[test]
fn others_affinity_on_the_instance_s_cpus_refuses_it() {
    let p = on(12, "others_affinity = \"0xF\"\n");
    assert_eq!(
        instance_room(&p, 1, "dasio-mbc-snv.exe"),
        Err("instance 1's CPUs 2-3 are pinned to others_affinity (0-3) too: move them off in the site profile first (several dasio on one PC must not fight, dasio#199)".into())
    );
    // On the system CPUs it leaves the instance its own.
    let system = on(12, "others_affinity = \"{system}\"\n");
    assert!(instance_room(&system, 1, "dasio-mbc-snv.exe").is_ok());
    // The instance's own entry never counts against it.
    let own = on(12, &entry("dasio-mbc-snv.exe", "0xC"));
    assert!(instance_room(&own, 1, "dasio-mbc-snv.exe").is_ok());
}

/// install.ps1 (PowerShell, run on the PCs only): the CPU step asks the
/// bundle's dasio for the instance's room, with the instance's exe, before
/// the NIC driver step, the profile's apply and the service stop, and any
/// refusal throws.
#[test]
fn install_ps1_refuses_an_instance_without_cpus_of_its_own_before_touching_anything() {
    let s = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install.ps1"),
    )
    .unwrap()
    // A Windows checkout has CRLF line ends.
    .replace("\r\n", "\n");
    let at = |needle: &str| {
        s.find(needle)
            .unwrap_or_else(|| panic!("install.ps1 has no {needle}"))
    };
    let name = at("$instExeName = \"dasio-$deviceName.exe\"");
    let guid = at("if ($profileGuid -ne $adapterGuid) {");
    let ask = at(
        "$room = @(& (Join-Path $here \"dasio.exe\") pc-profile instance-cpus --instance $Instance --exe $instExeName)",
    );
    let exit = at("$roomExit = $LASTEXITCODE");
    let refuse = at("if ($roomExit -ne 0) {");
    let nic = at("    if ($NicDriver) {");
    let apply = at("& (Join-Path $here \"dasio.exe\") pc-profile apply");
    let stop = at("Stop-Service -Name $service");
    assert!(name < ask, "the exe's name is known first");
    assert!(guid < ask && ask < exit && exit < refuse && refuse < nic);
    assert!(nic < apply && apply < stop);
    assert!(s[refuse..nic].contains("throw \"the CPU step: dasio refused instance $Instance"));
    // Inside the baseline step, which a test-clock install (CI's service
    // test, instance 14 without a site profile) skips.
    let baseline = at("$applied = 0\nif (-not $TestClock) {");
    assert!(baseline < ask);
}
