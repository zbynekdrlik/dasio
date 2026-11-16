//! The CPU layout computed from the CPU count (dasio#128).

use super::super::*;

#[test]
fn the_layout_is_what_foh_mbc_and_dsg_proved() {
    // FOH: 32 CPUs, band and master.
    let foh = Layout::of(32, 2);
    assert_eq!(foh.system_mask(), 0xF);
    assert_eq!(foh.rxd_cpu(), 3);
    assert_eq!(foh.dasio_mask(1), Ok(0xF0));
    assert_eq!(foh.dasio_mask(2), Ok(0xF00));
    assert_eq!(foh.daw_mask(), Ok(0xFFFF_F000));
    // mbc: 12 CPUs, one timer-waiting instance.
    let mbc = Layout::of(12, 1);
    assert_eq!(mbc.system_mask(), 0x3);
    assert_eq!(mbc.rxd_cpu(), 1);
    assert_eq!(mbc.dasio_mask(1), Ok(0xC));
    assert_eq!(mbc.daw_mask(), Ok(0xFF0));
    // dsg: 4 CPUs, the bridge.
    let dsg = Layout::of(4, 1);
    assert_eq!(dsg.system_mask(), 0x1);
    assert_eq!(dsg.rxd_cpu(), 0);
    assert_eq!(dsg.dasio_mask(1), Ok(0x6));
    assert_eq!(dsg.daw_mask(), Ok(0x8));
}

#[test]
fn the_system_takes_one_cpu_per_eight_and_an_instance_four_from_sixteen() {
    assert_eq!(Layout::of(8, 1).system, 1);
    assert_eq!(Layout::of(9, 1).system, 2);
    assert_eq!(Layout::of(16, 1).system, 2);
    assert_eq!(Layout::of(15, 1).per_instance, 2);
    assert_eq!(Layout::of(16, 1).per_instance, 4);
    // No CPU count read: one CPU, never zero system CPUs.
    assert_eq!(Layout::of(0, 1).system, 1);
    assert_eq!(Layout::of(0, 1).cpus, 1);
    // One processor group at most: the masks are 64 bits.
    let big = Layout::of(128, 1);
    assert_eq!(big.cpus, 64);
    assert_eq!(big.system_mask(), 0xFF);
    assert_eq!(big.daw_mask(), Ok(u64::MAX << 12));
}

#[test]
fn an_instance_or_a_daw_that_does_not_fit_is_named() {
    let small = Layout::of(4, 2);
    assert_eq!(small.dasio_mask(1), Ok(0x6));
    assert_eq!(
        small.dasio_mask(2),
        Err("dasio:2 needs CPUs 3-4, the PC has 4".into())
    );
    assert_eq!(
        small.daw_mask(),
        Err("no CPU is left for the DAW: 1 system and 2 x 2 dasio CPUs of 4".into())
    );
    let one = Layout::of(32, 1);
    assert_eq!(
        one.dasio_mask(2),
        Err("dasio:2: the profile has instances = 1 (1 to 1)".into())
    );
    assert!(one.dasio_mask(0).is_err());
    // Exactly full: the last instance fits, the DAW has none.
    let full = Layout::of(5, 2);
    assert_eq!(full.dasio_mask(2), Ok(0x18));
    assert!(full.daw_mask().is_err());
}

#[test]
fn the_layout_reads_as_one_line() {
    assert_eq!(
        Layout::of(32, 2).describe(),
        "system 0-3 (NIC interrupts, rxd 3), dasio:1 4-7, dasio:2 8-11, DAW 12-31"
    );
    assert_eq!(
        Layout::of(4, 1).describe(),
        "system 0 (NIC interrupts, rxd 0), dasio:1 1-2, DAW 3"
    );
    assert_eq!(
        Layout::of(4, 2).describe(),
        "system 0 (NIC interrupts, rxd 0), dasio:1 1-2, dasio:2 needs CPUs 3-4, the PC has 4, no CPU left for the DAW"
    );
    // The instances take the last CPU: none is left for the DAW.
    assert_eq!(
        Layout::of(5, 2).describe(),
        "system 0 (NIC interrupts, rxd 0), dasio:1 1-2, dasio:2 3-4, no CPU left for the DAW"
    );
}

#[test]
fn placeholders_stand_for_the_layout_s_masks() {
    let l = Layout::of(32, 2);
    assert_eq!(l.placeholder("{system}"), Some(Ok(0xF)));
    assert_eq!(l.placeholder(" {daw} "), Some(Ok(0xFFFF_F000)));
    assert_eq!(l.placeholder("{dasio:2}"), Some(Ok(0xF00)));
    assert!(matches!(l.placeholder("{dasio:3}"), Some(Err(_))));
    assert_eq!(l.placeholder("{dasio:x}"), None);
    assert_eq!(l.placeholder("{other}"), None);
    assert_eq!(l.placeholder("0xF0"), None);
    assert_eq!(l.placeholder("{system"), None);
    assert!(layout::affinity_ok("0xF0"));
    assert!(layout::affinity_ok("{dasio:2}"));
    assert!(!layout::affinity_ok("{cpu}"));
    assert!(!layout::affinity_ok("cpus 4-7"));
}

const COMPUTED: &str = r#"
name = "pc"
nic_guid = "{G}"
instances = 2

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\dasiopacer\Parameters'
name = "RxdCpu"
computed = "rxd_cpu"

[[registry]]
key = '{nic_device}\Interrupt Management\Affinity Policy'
name = "AssignmentSetOverride"
computed = "system_cpus"

[[process]]
exe = "dasio-band.exe"
affinity = "{dasio:1}"

[[process]]
exe = "Live.exe"
affinity = "{daw}"

[[process]]
exe = "dsync.exe"
affinity = "0xF"
"#;

#[test]
fn resolve_puts_the_layout_s_values_in() {
    let mut p = parse_with(COMPUTED, false).unwrap();
    assert_eq!(p.instances, Some(2));
    resolve(&mut p, 32).unwrap();
    let rxd = &p.registry[0];
    assert_eq!((rxd.dword, rxd.computed.as_deref()), (Some(3), None));
    assert_eq!(want_reg(rxd), Some(Value::Dword(3)));
    let irq = &p.registry[1];
    assert_eq!(irq.binary.as_deref(), Some("0f00000000000000"));
    assert_eq!(
        want_reg(irq),
        Some(Value::Binary(vec![0x0f, 0, 0, 0, 0, 0, 0, 0]))
    );
    let aff: Vec<_> = p.process.iter().map(|e| e.affinity.clone()).collect();
    assert_eq!(
        aff,
        [
            Some("0xF0".into()),
            Some("0xFFFFF000".into()),
            Some("0xF".into())
        ]
    );
}

#[test]
fn resolve_names_what_the_layout_cannot_fill() {
    let mut p = parse_with(COMPUTED, false).unwrap();
    assert_eq!(
        resolve(&mut p, 4),
        Err(
            "process Live.exe: no CPU is left for the DAW: 1 system and 2 x 2 dasio CPUs of 4"
                .into()
        )
    );
    // An unknown name parse refuses, resolve refuses too.
    let mut q: Profile = toml::from_str(
        "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\ncomputed = \"cpu\"\n",
    )
    .unwrap();
    assert!(
        resolve(&mut q, 8)
            .unwrap_err()
            .contains("computed cpu is not one of")
    );
}

#[test]
fn parse_checks_computed_values_and_placeholders() {
    let err = |t: &str| parse_with(&format!("name = \"x\"\n{t}"), false).unwrap_err();
    assert!(
        err("[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\ncomputed = \"cpu\"\n")
            .contains("computed cpu is not one of")
    );
    assert!(
        err("[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\ncomputed = \"rxd_cpu\"\ndword = 3\n")
            .contains("not both")
    );
    assert!(err("instances = 0\n").contains("instances must be 1 or more"));
    assert!(
        err("[[process]]\nexe = \"a.exe\"\naffinity = \"{cpus}\"\n")
            .contains("{system}, {dasio:N} or {daw}")
    );
    assert!(
        parse_with(
            "name = \"x\"\ninstances = 1\n[[process]]\nexe = \"a.exe\"\naffinity = \"{daw}\"\n",
            false
        )
        .is_ok()
    );
}

#[test]
fn the_shipped_baseline_computes_foh_s_nic_interrupts_and_rxd_cpu() {
    let mut b = baseline().unwrap();
    resolve(&mut b, 32).unwrap();
    let get = |name: &str| {
        b.registry
            .iter()
            .find(|e| e.name == name)
            .and_then(want_reg)
            .unwrap()
    };
    assert_eq!(get("RxdCpu"), Value::Dword(3));
    assert_eq!(
        get("AssignmentSetOverride"),
        Value::Binary(vec![0x0f, 0, 0, 0, 0, 0, 0, 0])
    );
    assert_eq!(get("DevicePolicy"), Value::Dword(4));
    // Only on the PCIe Realtek: a USB NIC's interrupts are its xHCI's.
    assert!(
        b.registry
            .iter()
            .filter(|e| e.key.contains("Affinity Policy"))
            .all(|e| e.nic_service.as_deref() == Some("rt640x64"))
    );
}

#[test]
fn load_resolves_the_layout_for_the_pc_s_cpu_count() {
    let dir = std::env::temp_dir().join(format!("dasio-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pc.toml");
    std::fs::write(&file, COMPUTED).unwrap();
    let p = load(&file, 12).unwrap();
    assert!(p.registry.iter().all(|e| e.computed.is_none()));
    let rxd = p.registry.iter().find(|e| e.name == "RxdCpu").unwrap();
    assert_eq!(rxd.dword, Some(1), "mbc's 12 CPUs: system 0-1");
    // Two instances on 4 CPUs leave the DAW none: refused, named.
    assert!(
        load(&file, 4)
            .unwrap_err()
            .contains("no CPU is left for the DAW")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A PC profile written before the layout (FOH's and dsg's until 0.1.107):
/// its own RxdCpu, NIC interrupt mask and hex affinities, no `instances`.
const OLDER: &str = r#"
name = "older"
nic_guid = "{G}"

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\dasiopacer\Parameters'
name = "RxdCpu"
dword = 2

[[registry]]
key = '{nic_device}\Interrupt Management\Affinity Policy'
name = "AssignmentSetOverride"
binary = "0300000000000000"

[[process]]
exe = "Live.exe"
affinity = "0xFFFFF000"
"#;

#[test]
fn an_older_profile_s_own_values_win_over_the_computed_ones() {
    let dir = std::env::temp_dir().join(format!("dasio-older-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pc.toml");
    std::fs::write(&file, OLDER).unwrap();
    let p = load(&file, 32).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(p.instances, None);
    let one = |name: &str| {
        let all: Vec<_> = p.registry.iter().filter(|e| e.name == name).collect();
        assert_eq!(
            all.len(),
            1,
            "{name}: the site's entry replaces the baseline's"
        );
        all[0]
    };
    // The site's values, not the layout's (rxd 3, mask 0xF on 32 CPUs).
    assert_eq!(want_reg(one("RxdCpu")), Some(Value::Dword(2)));
    assert_eq!(
        want_reg(one("AssignmentSetOverride")),
        Some(Value::Binary(vec![3, 0, 0, 0, 0, 0, 0, 0]))
    );
    // The baseline's other computed-free entries still come in.
    assert_eq!(want_reg(one("DevicePolicy")), Some(Value::Dword(4)));
    let live = p.process.iter().find(|e| e.exe == "Live.exe").unwrap();
    assert_eq!(live.affinity.as_deref(), Some("0xFFFFF000"));
    assert!(p.registry.iter().all(|e| e.computed.is_none()));
}

#[test]
fn the_reserved_cpus_are_every_instance_s_cpus() {
    assert_eq!(Layout::of(32, 2).dasio_cpus_mask(), 0xFF0, "FOH");
    assert_eq!(Layout::of(12, 1).dasio_cpus_mask(), 0xC, "mbc");
    assert_eq!(Layout::of(4, 1).dasio_cpus_mask(), 0x6, "dsg");
    // An instance that does not fit adds nothing: the profile still loads
    // (its placeholder names the problem); none fits, nothing reserved.
    assert_eq!(Layout::of(4, 2).dasio_cpus_mask(), 0x6);
    assert_eq!(Layout::of(2, 1).dasio_cpus_mask(), 0);
}

#[test]
fn the_shipped_baseline_reserves_dasio_s_cpus_after_a_reboot() {
    let get = |cpus: usize, instances: usize| {
        let mut b = baseline().unwrap();
        b.instances = Some(instances);
        resolve(&mut b, cpus).unwrap();
        let all: Vec<_> = b
            .registry
            .iter()
            .filter(|e| e.name == "ReservedCpuSets")
            .collect();
        assert_eq!(all.len(), 1);
        let e = all[0];
        assert_eq!(
            e.key,
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\kernel"
        );
        // Checked and applied, never a reason to refuse the start.
        assert!(!e.must && !e.check_only && !e.absent_ok);
        assert!(e.computed.is_none());
        want_reg(e).unwrap()
    };
    assert_eq!(
        get(32, 2),
        Value::Binary(vec![0xf0, 0x0f, 0, 0, 0, 0, 0, 0])
    );
    assert_eq!(get(12, 1), Value::Binary(vec![0x0c, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(get(4, 1), Value::Binary(vec![0x06, 0, 0, 0, 0, 0, 0, 0]));
    // A too small PC loads (nothing reserved), never refuses the profile.
    assert_eq!(get(2, 1), Value::Binary(vec![0; 8]));
    // apply says it takes a reboot.
    let a = Action::Reg {
        key: r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\kernel".into(),
        name: "ReservedCpuSets".into(),
        value: get(4, 1),
    };
    assert_eq!(takes_effect(&a), "a reboot");
}

#[test]
fn a_site_s_own_reserved_cpus_win_over_the_baseline_s() {
    let dir = std::env::temp_dir().join(format!("dasio-reserved-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pc.toml");
    std::fs::write(
        &file,
        "name = \"foh\"\ninstances = 2\n[[registry]]\nkey = 'HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\kernel'\nname = \"ReservedCpuSets\"\nbinary = \"f0ffffff00000000\"\n",
    )
    .unwrap();
    let p = load(&file, 32).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let all: Vec<_> = p
        .registry
        .iter()
        .filter(|e| e.name == "ReservedCpuSets")
        .collect();
    assert_eq!(all.len(), 1, "the site's entry replaces the baseline's");
    assert_eq!(
        want_reg(all[0]),
        Some(Value::Binary(vec![0xf0, 0xff, 0xff, 0xff, 0, 0, 0, 0]))
    );
}

#[test]
fn site_instances_reads_the_site_file_or_one() {
    let dir = std::env::temp_dir().join(format!("dasio-instances-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pc.toml");
    assert_eq!(site_instances(&file), Ok(1), "no profile: one instance");
    std::fs::write(&file, "name = \"foh\"\ninstances = 2\n").unwrap();
    assert_eq!(site_instances(&file), Ok(2));
    std::fs::write(&file, "name = \"mbc\"\n").unwrap();
    assert_eq!(site_instances(&file), Ok(1));
    std::fs::write(&file, "name = \"x\"\ninstances = 0\n").unwrap();
    assert!(
        site_instances(&file)
            .unwrap_err()
            .contains("instances must be 1")
    );
    // A profile path that cannot be read (a directory): an error, not 1.
    assert!(site_instances(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
