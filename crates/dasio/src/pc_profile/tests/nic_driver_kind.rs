//! `[[nic_driver]]` (dasio#128): the Dante NIC family's pinned driver
//! package, checked by hardware ID and verified before install.ps1
//! installs it.

use super::super::nic_driver::{
    NicDriverEntry, Package, PackageFile, check, command, common_name, id_matches, is_catalog,
    pick, validate, verify,
};
use super::super::*;
use super::Pc;

const INF: &str = "BA0F860540FAE983DFDB803EC306C26128F43D4E1C5555B97521561A5840A9EC";
const CAT: &str = "EA8CC8DED3C8CAAA3AECFB96B49FC4E819A8B0499450470A6A231C4E2437BBC5";
const SYS: &str = "DBFD463C760F47862EC5682695BD96E3D3F71EFD7C21109FC5D3EA0927137117";
const SIGNER: &str = "CN=Microsoft Windows Hardware Compatibility Publisher, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";
const FOH_ID: &str = r"PCI\VEN_10EC&DEV_8125&SUBSYS_E0001458&REV_05";
const DSG_ID: &str = r"PCI\VEN_10EC&DEV_8168&SUBSYS_189B1025&REV_15";

fn file(name: &str, sha256: &str) -> PackageFile {
    PackageFile {
        name: name.into(),
        sha256: sha256.into(),
    }
}

/// The shipped Realtek entry's shape.
fn entry() -> NicDriverEntry {
    NicDriverEntry {
        family: "Realtek PCIe (rt640x64)".into(),
        hardware_ids: vec![
            r"PCI\VEN_10EC&DEV_8125".into(),
            r"PCI\VEN_10EC&DEV_8168".into(),
        ],
        version: "10.79.20.1003".into(),
        provider: "Realtek".into(),
        inf: "rt640x64sta.inf".into(),
        signer: "Microsoft Windows Hardware Compatibility Publisher".into(),
        files: vec![
            file("rt640x64sta.inf", INF),
            file("rt640x64.cat", CAT),
            file("rt640x64.sys", SYS),
        ],
        source: "FOH's driver store".into(),
        why: "measured best".into(),
    }
}

/// The package exactly as pinned.
fn package() -> Package {
    Package {
        files: vec![
            ("rt640x64.sys".into(), SYS.to_ascii_lowercase()),
            ("RT640X64STA.INF".into(), INF.into()),
            ("rt640x64.cat".into(), CAT.into()),
        ],
        signature: "Valid".into(),
        signer: SIGNER.into(),
    }
}

/// A PC whose Dante NIC has `ids` and the class key's version/provider.
fn pc(ids: &[&str], version: Option<&str>, provider: Option<&str>) -> Pc {
    let mut p = Pc {
        hardware_ids: ids.iter().map(|s| s.to_string()).collect(),
        ..Pc::default()
    };
    for (name, v) in [("DriverVersion", version), ("ProviderName", provider)] {
        if let Some(v) = v {
            p.reg
                .insert(("{nic_class}".into(), name.into()), Value::String(v.into()));
        }
    }
    p
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_shipped_baseline_pins_foh_s_realtek_10_79_package() {
    let b = baseline().unwrap();
    assert_eq!(b.nic_driver.len(), 1);
    let e = &b.nic_driver[0];
    assert_eq!(validate(e), Ok(()));
    assert_eq!(e.hardware_ids, entry().hardware_ids);
    assert_eq!(e.version, "10.79.20.1003");
    assert_eq!(e.provider, "Realtek");
    assert_eq!(e.inf, "rt640x64sta.inf");
    assert_eq!(
        e.signer,
        "Microsoft Windows Hardware Compatibility Publisher"
    );
    assert_eq!(e.files, entry().files);
    // FOH's own NIC and dsg's both fall under it.
    assert!(pick(&b.nic_driver, &[FOH_ID.to_string()]).is_some());
    assert!(pick(&b.nic_driver, &[DSG_ID.to_string()]).is_some());
    // FOH's package, read on FOH 17.11.2026, is the pinned one.
    assert_eq!(verify(e, &package()), Vec::<String>::new());
}

#[test]
fn a_valid_entry_passes_and_a_usb_prefix_is_a_hardware_id_too() {
    assert_eq!(validate(&entry()), Ok(()));
    let mut e = entry();
    e.hardware_ids = vec![r"usb\vid_0bda&pid_8156".into()];
    assert_eq!(validate(&e), Ok(()));
    e.hardware_ids = vec![r"pci\ven_10ec&dev_8125&subsys_e0001458".into()];
    assert_eq!(validate(&e), Ok(()));
}

#[test]
fn an_entry_missing_what_install_needs_is_refused_naming_it() {
    let f = r#"nic_driver "Realtek PCIe (rt640x64)": "#;
    let refused = |change: &dyn Fn(&mut NicDriverEntry), why: &str| {
        let mut e = entry();
        change(&mut e);
        assert_eq!(validate(&e), Err(format!("{f}{why}")));
    };
    refused(
        &|e| e.hardware_ids.clear(),
        "give at least one hardware_ids prefix",
    );
    let not_device = r"is not a PCI\VEN_...&DEV_... or USB\VID_...&PID_... prefix";
    refused(
        &|e| e.hardware_ids.push("VEN_10EC&DEV_8125".into()),
        &format!(r#"hardware id "VEN_10EC&DEV_8125" {not_device}"#),
    );
    refused(
        &|e| e.hardware_ids[1] = r"PCI\DEV_8125".into(),
        &format!(r#"hardware id "PCI\\DEV_8125" {not_device}"#),
    );
    // A vendor alone would take every NIC of it.
    refused(
        &|e| e.hardware_ids[1] = r"PCI\VEN_10EC".into(),
        &format!(r#"hardware id "PCI\\VEN_10EC" {not_device}"#),
    );
    refused(
        &|e| e.hardware_ids[1] = r"USB\VID_0BDA&DEV_8156".into(),
        &format!(r#"hardware id "USB\\VID_0BDA&DEV_8156" {not_device}"#),
    );
    refused(
        &|e| e.hardware_ids[1] = r"PCI\VEN_10EC&PID_8125".into(),
        &format!(r#"hardware id "PCI\\VEN_10EC&PID_8125" {not_device}"#),
    );
    refused(
        &|e| e.hardware_ids[1] = r"USB\PID_8156&VID_0BDA".into(),
        &format!(r#"hardware id "USB\\PID_8156&VID_0BDA" {not_device}"#),
    );
    refused(&|e| e.version = " ".into(), "version must not be empty");
    refused(
        &|e| e.provider = String::new(),
        "provider must not be empty",
    );
    refused(&|e| e.signer = String::new(), "signer must not be empty");
    refused(&|e| e.source = String::new(), "source must not be empty");
    refused(&|e| e.why = String::new(), "why must not be empty");
    refused(
        &|e| e.files[1].name = r"x\rt640x64.cat".into(),
        r#"file "x\\rt640x64.cat": give the name alone, no folder"#,
    );
    refused(
        &|e| e.files[2].name = "a/b.sys".into(),
        r#"file "a/b.sys": give the name alone, no folder"#,
    );
    refused(
        &|e| e.files[2].name = "c:b.sys".into(),
        r#"file "c:b.sys": give the name alone, no folder"#,
    );
    refused(
        &|e| e.files[2].name = "..".into(),
        r#"file "..": give the name alone, no folder"#,
    );
    refused(
        &|e| e.files[2].name = String::new(),
        r#"file "": give the name alone, no folder"#,
    );
    refused(
        &|e| {
            e.files[2].sha256.pop();
        },
        "file rt640x64.sys: sha256 is not 64 hex digits",
    );
    refused(
        &|e| e.files[2].sha256.push('0'),
        "file rt640x64.sys: sha256 is not 64 hex digits",
    );
    refused(
        &|e| e.files[2].sha256.replace_range(10..11, "g"),
        "file rt640x64.sys: sha256 is not 64 hex digits",
    );
    refused(
        &|e| e.files.push(file("RT640X64.SYS", SYS)),
        "file RT640X64.SYS is named twice",
    );
    refused(
        &|e| e.inf = "other.inf".into(),
        "inf other.inf is not one of its files",
    );
    refused(
        &|e| e.inf = "rt640x64.sys".into(),
        "inf rt640x64.sys is not an .inf file",
    );
    refused(
        &|e| e.files.remove(1).name.clear(),
        "give exactly one .cat file, not 0",
    );
    refused(
        &|e| e.files.push(file("second.cat", CAT)),
        "give exactly one .cat file, not 2",
    );
    let mut e = entry();
    e.family = " ".into();
    assert_eq!(
        validate(&e),
        Err(r#"nic_driver " ": family must not be empty"#.into())
    );
}

#[test]
fn a_profile_with_a_broken_nic_driver_entry_does_not_load() {
    let text = format!(
        "name = \"x\"\n[[nic_driver]]\nfamily = \"f\"\nhardware_ids = []\nversion = \"1\"\nprovider = \"p\"\ninf = \"a.inf\"\nsigner = \"s\"\nsource = \"there\"\n[[nic_driver.files]]\nname = \"a.inf\"\nsha256 = \"{INF}\"\n"
    );
    assert_eq!(
        parse(&text),
        Err("nic_driver \"f\": give at least one hardware_ids prefix".into())
    );
}

#[test]
fn a_catalog_is_a_cat_file_by_its_name() {
    assert!(is_catalog("rt640x64.cat"));
    assert!(is_catalog("RT640X64.CAT"));
    assert!(!is_catalog("rt640x64.sys"));
    assert!(!is_catalog("cat"));
}

#[test]
fn a_hardware_id_matches_its_prefix_only_at_a_boundary() {
    let p = r"PCI\VEN_10EC&DEV_8125";
    assert!(id_matches(p, p));
    assert!(id_matches(p, FOH_ID));
    assert!(id_matches(p, &FOH_ID.to_ascii_lowercase()));
    assert!(id_matches(p, r"PCI\VEN_10EC&DEV_8125\3&11583659&0&E4"));
    assert!(!id_matches(p, r"PCI\VEN_10EC&DEV_81250"));
    assert!(!id_matches(p, r"PCI\VEN_10EC&DEV_812"));
    assert!(!id_matches(p, DSG_ID));
    assert!(!id_matches(p, ""));
    // A multi-byte character right at the prefix's end is no panic.
    assert!(!id_matches(p, r"PCI\VEN_10EC&DEV_812é"));
}

#[test]
fn pick_takes_the_first_entry_any_of_whose_prefixes_takes_any_id() {
    let mut other = entry();
    other.family = "other".into();
    other.hardware_ids = vec![r"PCI\VEN_8086&DEV_15F3".into()];
    let entries = [other, entry()];
    let ids = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // The NIC's second ID matches the second entry's second prefix.
    let got = pick(&entries, &ids(&[r"PCI\VEN_1234&DEV_0001", DSG_ID])).unwrap();
    assert_eq!(got.family, "Realtek PCIe (rt640x64)");
    let got = pick(&entries, &ids(&[r"PCI\VEN_8086&DEV_15F3&SUBSYS_1"])).unwrap();
    assert_eq!(got.family, "other");
    assert!(pick(&entries, &ids(&[r"PCI\VEN_1234&DEV_0001"])).is_none());
    assert!(pick(&entries, &[]).is_none());
}

#[test]
fn the_check_warns_when_the_nic_runs_another_driver_than_its_family_s() {
    let e = [entry()];
    let fine = pc(&[FOH_ID], Some("10.79.20.1003"), Some("Realtek"));
    assert_eq!(check(&e, &fine), Vec::new());
    let want = "10.79.20.1003 from Realtek (rt640x64sta.inf)";
    let why = "measured best; install it with install.ps1 -NicDriver <folder> (someone on site): FOH's driver store";
    let one = |r: &Pc, found: &str| {
        let got = check(&e, r);
        assert_eq!(
            got,
            vec![Problem {
                what: "the Dante NIC's driver (Realtek PCIe (rt640x64))".into(),
                found: found.into(),
                want: want.into(),
                why: why.into(),
                must: false,
            }]
        );
        assert_eq!(
            got[0].to_string(),
            format!(
                "the Dante NIC's driver (Realtek PCIe (rt640x64)): {found} (the profile wants {want}) — {why}"
            )
        );
    };
    // dsg: Realtek's newer 10.80.
    one(
        &pc(&[DSG_ID], Some("10.80.20.407"), Some("Realtek")),
        "10.80.20.407 from Realtek",
    );
    // dsg before 15.11: Microsoft's inbox driver (rtcx21).
    one(
        &pc(&[DSG_ID], Some("1.0.0.14"), Some("Microsoft")),
        "1.0.0.14 from Microsoft",
    );
    // The right version from another provider.
    one(
        &pc(&[FOH_ID], Some("10.79.20.1003"), Some("Microsoft")),
        "10.79.20.1003 from Microsoft",
    );
    one(&pc(&[FOH_ID], None, Some("Realtek")), "absent from Realtek");
    let mut dword = pc(&[FOH_ID], None, Some("Realtek"));
    dword.reg.insert(
        ("{nic_class}".into(), "DriverVersion".into()),
        Value::Dword(7),
    );
    one(&dword, "Dword(7) from Realtek");
    let mut locked = pc(&[FOH_ID], None, None);
    locked.unreadable.push("{nic_class}".into());
    one(
        &locked,
        "unreadable (access denied) from unreadable (access denied)",
    );
    // A NIC of no listed family, or none found: nothing to say here.
    assert_eq!(
        check(
            &e,
            &pc(&[r"PCI\VEN_8086&DEV_15F3"], Some("1"), Some("Intel"))
        ),
        Vec::new()
    );
    assert_eq!(check(&e, &pc(&[], Some("1"), Some("x"))), Vec::new());
}

#[test]
fn the_shipped_baseline_reports_a_rtl8168_on_the_inbox_driver() {
    // dsg 15.11.2026 before Realtek's installer: rtcx21 1.0.0.14.
    let mut p = baseline_only(&baseline().unwrap());
    p.registry.clear();
    let problems = super::super::check(&p, &pc(&[DSG_ID], Some("1.0.0.14"), Some("Microsoft")));
    let texts: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("the Dante NIC's driver (Realtek PCIe (rt640x64)): 1.0.0.14 from Microsoft (the profile wants 10.79.20.1003 from Realtek (rt640x64sta.inf))")),
        "{texts:#?}"
    );
    // A WARN, never must-have.
    assert!(
        problems
            .iter()
            .filter(|p| p.what.starts_with("the Dante NIC's driver"))
            .all(|p| !p.must)
    );
}

#[test]
fn the_package_check_names_every_difference() {
    let e = entry();
    assert_eq!(verify(&e, &package()), Vec::<String>::new());
    let mut p = package();
    p.files.remove(0);
    p.files[1].1 = SYS.into();
    p.files.push(("setup.exe".into(), SYS.into()));
    p.signature = "HashMismatch".into();
    p.signer = "CN=Someone, O=Microsoft Windows Hardware Compatibility Publisher".into();
    assert_eq!(
        verify(&e, &p),
        vec![
            format!("rt640x64.cat: SHA256 {SYS}, the baseline pins {CAT}"),
            "rt640x64.sys: missing".to_string(),
            "setup.exe: not a file of the pinned package".to_string(),
            "the catalog's signature is \"HashMismatch\", not Valid".to_string(),
            "the catalog is signed by \"CN=Someone, O=Microsoft Windows Hardware Compatibility Publisher\", the baseline pins CN=Microsoft Windows Hardware Compatibility Publisher".to_string(),
        ]
    );
    // Case is no difference; a subject with no CN is.
    let mut p = package();
    p.signature = "valid".into();
    p.signer = "cn=microsoft windows hardware compatibility publisher".into();
    assert_eq!(
        verify(&e, &p),
        vec!["the catalog is signed by \"cn=microsoft windows hardware compatibility publisher\", the baseline pins CN=Microsoft Windows Hardware Compatibility Publisher".to_string()]
    );
    p.signer = "CN=microsoft windows hardware compatibility publisher".into();
    assert_eq!(verify(&e, &p), Vec::<String>::new());
}

#[test]
fn a_subject_s_common_name_is_its_cn_part() {
    assert_eq!(
        common_name(SIGNER),
        Some("Microsoft Windows Hardware Compatibility Publisher")
    );
    assert_eq!(common_name("O=x, CN=y, C=US"), Some("y"));
    assert_eq!(common_name("O=x"), None);
    assert_eq!(common_name(""), None);
}

/// install.ps1's arguments for `package()` on FOH's NIC.
fn full() -> Vec<String> {
    let mut a = args(&[
        "--hardware-id",
        FOH_ID,
        "--hardware-id",
        "PCI\\VEN_10EC&DEV_8125",
    ]);
    for (n, h) in package().files {
        a.push("--file".into());
        a.push(format!("{n}={h}"));
    }
    a.extend(args(&["--signature", "Valid", "--signer", SIGNER]));
    a
}

#[test]
fn nic_driver_command_names_the_inf_to_install_for_the_pinned_package() {
    assert_eq!(
        command(&[entry()], &full()),
        (
            vec![
                "the package is Realtek PCIe (rt640x64)'s pinned driver (rt640x64sta.inf 10.79.20.1003, signed by Microsoft Windows Hardware Compatibility Publisher)".to_string(),
                "INF rt640x64sta.inf".to_string(),
                "VERSION 10.79.20.1003".to_string(),
            ],
            0
        )
    );
}

#[test]
fn nic_driver_command_refuses_another_package_or_nic() {
    let mut a = full();
    let at = a.iter().position(|x| x == "Valid").unwrap();
    a[at] = "NotSigned".into();
    assert_eq!(
        command(&[entry()], &a),
        (
            vec!["REFUSED this is not Realtek PCIe (rt640x64)'s pinned package (rt640x64sta.inf 10.79.20.1003 from FOH's driver store): the catalog's signature is \"NotSigned\", not Valid".to_string()],
            2
        )
    );
    let a = args(&[
        "--hardware-id",
        r"PCI\VEN_8086&DEV_15F3",
        "--hardware-id",
        "USB\\VID_0BDA",
        "--file",
        "a.inf=00",
        "--signature",
        "Valid",
        "--signer",
        SIGNER,
    ]);
    assert_eq!(
        command(&[entry()], &a),
        (
            vec![r"REFUSED the Dante NIC (PCI\VEN_8086&DEV_15F3, USB\VID_0BDA) is of no family the baseline pins a driver for".to_string()],
            2
        )
    );
}

#[test]
fn nic_driver_command_refuses_arguments_it_cannot_read() {
    let e = [entry()];
    let bad = |a: &[&str], why: &str| {
        assert_eq!(command(&e, &args(a)), (vec![why.to_string()], 64));
    };
    bad(&["--hardware-id"], "--hardware-id: needs a value");
    bad(
        &["--file", "noequals"],
        "--file noequals: give <name>=<sha256>",
    );
    bad(&["--file", "=abc"], "--file =abc: give <name>=<sha256>");
    bad(&["--plan", "x"], "--plan: not an argument of nic-driver");
    bad(
        &["--hardware-id", FOH_ID, "--file", "a=b", "--signer", "CN=x"],
        "give --signature and --signer",
    );
    bad(
        &[
            "--hardware-id",
            FOH_ID,
            "--file",
            "a=b",
            "--signature",
            "Valid",
        ],
        "give --signature and --signer",
    );
    bad(
        &["--file", "a=b", "--signature", "Valid", "--signer", "CN=x"],
        "give --hardware-id and --file",
    );
    bad(
        &[
            "--hardware-id",
            FOH_ID,
            "--signature",
            "Valid",
            "--signer",
            "CN=x",
        ],
        "give --hardware-id and --file",
    );
}

#[test]
fn a_site_nic_driver_entry_replaces_the_baseline_s_same_family() {
    let mut base = baseline_only(&baseline().unwrap());
    base.nic_driver = vec![entry()];
    let mut site = base.clone();
    site.registry.clear();
    site.ini.clear();
    site.process.clear();
    site.disk.clear();
    site.bcd.clear();
    site.task.clear();
    site.absent.clear();
    let mut dsg = entry();
    dsg.family = "REALTEK PCIE (RT640X64)".into();
    dsg.version = "10.80.20.407".into();
    let mut intel = entry();
    intel.family = "Intel".into();
    site.nic_driver = vec![dsg.clone(), intel.clone()];
    let merged = merge(&base, &site);
    assert_eq!(merged.nic_driver, vec![dsg.clone(), intel]);
    site.nic_driver = vec![];
    assert_eq!(merge(&base, &site).nic_driver, vec![entry()]);
    // Counted with the rest; kept for the in-process check (registry
    // reads only).
    assert_eq!(merged.entries(), base.entries() + 1);
    assert_eq!(in_process(&merged).nic_driver, merged.nic_driver);
}

#[test]
fn load_merged_is_the_baseline_alone_without_a_site_file_and_merged_with_one() {
    let dir = std::env::temp_dir().join(format!("dasio-nicdrv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("none.toml");
    let base = baseline().unwrap();
    assert_eq!(load_merged(&missing).unwrap(), baseline_only(&base));
    let site = dir.join("pc-profile.toml");
    std::fs::write(&site, "name = \"dsg\"\n").unwrap();
    let got = load_merged(&site).unwrap();
    std::fs::write(&site, "name = [").unwrap();
    let broken = load_merged(&site).unwrap_err();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got.name, "dsg");
    assert_eq!(got.nic_driver, base.nic_driver);
    assert!(broken.starts_with(&site.display().to_string()), "{broken}");
}
