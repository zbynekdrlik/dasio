//! Must-have items of the baseline (dasio#128): the device does not run
//! without them.

use super::super::*;
use super::Pc;

const MUSTS: &str = r#"
name = "b"

[[registry]]
key = '{nic_class}'
name = "*InterruptModeration"
string = "0"
nic_service = "rt640x64"
must = true

[[registry]]
key = '{nic_class}'
name = "*WakeOnPattern"
string = "0"
nic_service = "rt640x64"

[[registry]]
key = 'HKLM\A'
name = "X"
dword = 1

[[bcd]]
name = "testsigning"
value = "Yes"
must = true
"#;

fn realtek() -> Pc {
    Pc {
        nic_service: Some("rt640x64".into()),
        ..Pc::default()
    }
}

fn texts(p: &[Problem]) -> Vec<String> {
    p.iter().map(ToString::to_string).collect()
}

#[test]
fn a_must_have_problem_says_so_and_the_rest_do_not() {
    let base = parse_with(MUSTS, false).unwrap();
    let p = merge(
        &base,
        &parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap(),
    );
    let found = check(&p, &realtek());
    let must: Vec<bool> = found.iter().map(|x| x.must).collect();
    // InterruptModeration, WakeOnPattern, X, testsigning.
    assert_eq!(must, [true, false, false, true], "{found:?}");
    let t = texts(&found);
    assert!(t[0].starts_with("must-have: registry {nic_class}\\*InterruptModeration: absent"));
    assert!(t[1].starts_with("registry {nic_class}\\*WakeOnPattern"));
    assert!(t[3].starts_with("must-have: bcd testsigning: absent"));
}

#[test]
fn must_have_failure_names_every_must_have_item_and_nothing_else() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(must_have_failure(&s(&["registry A: absent"])), None);
    assert_eq!(must_have_failure(&[]), None);
    let why = must_have_failure(&s(&[
        "must-have: registry A: absent",
        "registry B: absent",
        "must-have: bcd testsigning: \"No\"",
    ]))
    .unwrap();
    assert_eq!(
        why,
        "the PC misses 2 must-have item(s) of dasio's baseline (dasio#128): registry A: absent; bcd testsigning: \"No\"; set them (dasio pc-profile apply, then restart the Dante NIC) and start dasio again"
    );
}

#[test]
fn a_site_value_for_a_must_have_item_stays_must_have() {
    let base = parse_with(MUSTS, false).unwrap();
    let pc = parse(
        "name = \"pc\"\nnic_guid = \"{G}\"\n[[registry]]\nkey = '{NIC_CLASS}'\nname = \"*interruptmoderation\"\nstring = \"1\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"X\"\ndword = 2\n[[bcd]]\nname = \"TESTSIGNING\"\nvalue = \"Yes\"\n",
    )
    .unwrap();
    let m = merge(&base, &pc);
    let im = m
        .registry
        .iter()
        .find(|e| e.name == "*interruptmoderation")
        .unwrap();
    assert!(im.must && im.string.as_deref() == Some("1"));
    let x = m.registry.iter().find(|e| e.name == "X").unwrap();
    assert!(!x.must, "a plain baseline entry stays plain");
    assert!(m.bcd[0].must, "the site's testsigning stays must-have");
    // A site value for a plain baseline bcd element stays plain.
    let plain = parse_with(
        "name = \"b\"\n[[bcd]]\nname = \"disabledynamictick\"\nvalue = \"Yes\"\n",
        false,
    )
    .unwrap();
    let m2 = merge(
        &plain,
        &parse("name = \"pc\"\n[[bcd]]\nname = \"disabledynamictick\"\nvalue = \"No\"\n").unwrap(),
    );
    assert_eq!(m2.bcd.len(), 1);
    assert!(!m2.bcd[0].must);
    // A site's own must-have item stays one with no baseline twin.
    let own = merge(
        &base,
        &parse(
            "name = \"pc\"\n[[registry]]\nkey = 'HKLM\\Z'\nname = \"Z\"\ndword = 1\nmust = true\n",
        )
        .unwrap(),
    );
    assert!(own.registry.iter().any(|e| e.name == "Z" && e.must));
}

#[test]
fn a_missing_dante_nic_is_must_have_only_with_must_have_nic_items() {
    let base = parse_with(MUSTS, false).unwrap();
    let p = merge(
        &base,
        &parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap(),
    );
    let found = check(&p, &Pc::default());
    assert!(found[0].what.starts_with("the Dante NIC {G}"));
    assert!(found[0].must);
    let plain = parse_with(
        "name = \"b\"\nnic_guid = \"{G}\"\n[[registry]]\nkey = '{nic_class}'\nname = \"*EEE\"\nstring = \"0\"\nnic_service = \"rt640x64\"\n",
        false,
    )
    .unwrap();
    let found = check(&plain, &Pc::default());
    assert!(found[0].what.starts_with("the Dante NIC {G}") && !found[0].must);
    // A must-have item that is not a NIC family's does not make it one.
    let other = parse_with(
        "name = \"b\"\nnic_guid = \"{G}\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"X\"\ndword = 1\nmust = true\n[[registry]]\nkey = '{nic_class}'\nname = \"*EEE\"\nstring = \"0\"\nnic_service = \"rt640x64\"\n",
        false,
    )
    .unwrap();
    let found = check(&other, &Pc::default());
    assert!(found[0].what.starts_with("the Dante NIC {G}") && !found[0].must);
}

#[test]
fn a_set_must_have_item_is_no_problem() {
    let base = parse_with(MUSTS, false).unwrap();
    let p = merge(
        &base,
        &parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap(),
    );
    let mut pc = realtek();
    pc.reg.insert(
        ("{nic_class}".into(), "*InterruptModeration".into()),
        Value::String("0".into()),
    );
    pc.bcd = vec![("testsigning".into(), "Yes".into())];
    let t = texts(&check(&p, &pc));
    assert_eq!(must_have_failure(&t), None, "{t:?}");
    assert_eq!(t.len(), 2, "the two plain entries: {t:?}");
}

#[test]
fn the_shipped_baseline_s_must_haves_are_the_nic_s_timing_and_test_signing() {
    let b = baseline().unwrap();
    let must = |name: &str| b.registry.iter().find(|e| e.name == name).unwrap().must;
    for n in [
        "*InterruptModeration",
        "*EEE",
        "*FlowControl",
        "*LsoV2IPv4",
        "*ReceiveBuffers",
        "*RSS",
        "EnableGreenEthernet",
    ] {
        assert!(must(n), "{n}");
    }
    // A wake setting, the driver's version and Windows settings are WARN.
    for n in ["*WakeOnPattern", "DriverVersion", "MMCSS", "RxdEnable"] {
        if let Some(e) = b.registry.iter().find(|e| e.name == n) {
            assert!(!e.must, "{n}");
        }
    }
    // Every must-have registry entry is on the Dante NIC's own keys: one
    // driver family's setting, or a standard NDIS keyword every NIC reads
    // the same (LSO, dasio#24). A driver-free one accepts an absent
    // keyword: one driver's keyword must-have on every NIC would stop the
    // device on any other NIC.
    assert!(
        b.registry
            .iter()
            .filter(|e| e.must)
            .all(|e| on_nic(&e.key) && (e.nic_service.is_some() || e.absent_ok))
    );
    assert!(b.bcd.iter().any(|e| e.name == "testsigning" && e.must));
    assert!(
        !b.bcd
            .iter()
            .any(|e| e.name == "disabledynamictick" && e.must)
    );
}

/// LSO is a standard NDIS keyword: the baseline holds it off on the Dante
/// NIC whatever its driver (mbc's USB Realtek ran it on while the check
/// read fine, 19.11.2026, dasio#24), as a must-have, and a NIC without LSO
/// (no keyword) is fine.
#[test]
fn the_baseline_holds_lso_off_on_every_dante_nic() {
    let mut lso = baseline().unwrap();
    lso.registry.retain(|e| e.name.starts_with("*LsoV2IPv"));
    let only = Profile {
        registry: lso.registry,
        ..parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap()
    };
    assert_eq!(only.registry.len(), 2, "{:?}", only.registry);
    let mut usb = Pc {
        nic_service: Some("rtu56cx22x64".into()),
        ..Pc::default()
    };
    let set = |pc: &mut Pc, v: &str| {
        for name in ["*LsoV2IPv4", "*LsoV2IPv6"] {
            pc.reg
                .insert(("{nic_class}".into(), name.into()), Value::String(v.into()));
        }
    };
    // A NIC without LSO has no keyword.
    assert_eq!(texts(&check(&only, &usb)), Vec::<String>::new());
    set(&mut usb, "1");
    let on = texts(&check(&only, &usb));
    assert_eq!(on.len(), 2, "{on:?}");
    for (t, name) in on.iter().zip(["*LsoV2IPv4", "*LsoV2IPv6"]) {
        assert!(
            t.starts_with(&format!("must-have: registry {{nic_class}}\\{name}: \"1\"")),
            "{t}"
        );
    }
    assert!(must_have_failure(&on).is_some());
    set(&mut usb, "0");
    assert_eq!(texts(&check(&only, &usb)), Vec::<String>::new());
    // FOH's and dsg's PCIe Realtek too.
    let mut pcie = realtek();
    set(&mut pcie, "1");
    assert_eq!(texts(&check(&only, &pcie)).len(), 2);
}

/// EEE off on every Dante NIC, whatever its driver (owner 20.11.2026: EEE
/// on any rig device is a serious fail; mbc's USB Realtek had it on while
/// the baseline's entries were scoped to rt640x64, dasio#24): `*EEE`,
/// Realtek's `AdvancedEEE` and `EnableGreenEthernet`, must-have, a NIC
/// without the keyword fine.
#[test]
fn the_baseline_holds_eee_off_on_every_dante_nic() {
    let names = ["*EEE", "AdvancedEEE", "EnableGreenEthernet"];
    let mut b = baseline().unwrap();
    b.registry.retain(|e| names.contains(&e.name.as_str()));
    let only = Profile {
        registry: b.registry,
        ..parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap()
    };
    assert_eq!(only.registry.len(), 3, "{:?}", only.registry);
    let mut usb = Pc {
        nic_service: Some("rtu56cx22x64".into()),
        ..Pc::default()
    };
    assert_eq!(texts(&check(&only, &usb)), Vec::<String>::new());
    for n in names {
        usb.reg
            .insert(("{nic_class}".into(), n.into()), Value::String("1".into()));
    }
    let on = texts(&check(&only, &usb));
    assert_eq!(on.len(), 3, "{on:?}");
    assert!(
        on.iter()
            .all(|t| t.starts_with("must-have: registry {nic_class}\\")),
        "{on:?}"
    );
    assert_eq!(plan(&only, &usb).len(), 3);
}

/// A Dante NIC the PC does not have is ONE must-have problem: the entries
/// on its keys are neither read one by one (Windows reads each as
/// unreadable, the driver-free LSO entries too) nor written (dasio#24).
#[test]
fn a_missing_dante_nic_is_one_problem_with_driver_free_nic_entries() {
    let mut nic = baseline().unwrap();
    nic.registry.retain(|e| on_nic(&e.key));
    assert!(
        nic.registry
            .iter()
            .any(|e| e.nic_service.is_none() && e.must)
    );
    let only = Profile {
        registry: nic.registry,
        ..parse("name = \"pc\"\nnic_guid = \"{G}\"\n").unwrap()
    };
    let gone = Pc {
        nic_gone: true,
        ..Pc::default()
    };
    let found = check(&only, &gone);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].what, "the Dante NIC {G}");
    assert!(found[0].must);
    assert_eq!(plan(&only, &gone), Vec::new());
    // With the NIC there, they are read and written again.
    let mut usb = Pc {
        nic_service: Some("rtu56cx22x64".into()),
        ..Pc::default()
    };
    usb.reg.insert(
        ("{nic_class}".into(), "*LsoV2IPv4".into()),
        Value::String("1".into()),
    );
    let t = texts(&check(&only, &usb));
    assert_eq!(t.len(), 1, "{t:?}");
    assert!(
        t[0].starts_with("must-have: registry {nic_class}\\*LsoV2IPv4: \"1\""),
        "{t:?}"
    );
    assert_eq!(
        plan(&only, &usb),
        vec![Action::Reg {
            key: "{nic_class}".into(),
            name: "*LsoV2IPv4".into(),
            value: Value::String("0".into()),
        }]
    );
}

/// FOH's site profile overrides every NIC-family entry with a driver-free
/// copy. A Dante NIC not found is still one line there, must-have through
/// the driver-free must-haves alone, and an entry off the NIC's keys is
/// checked and planned as ever (dasio#24).
#[test]
fn a_missing_dante_nic_is_one_line_with_driver_free_site_copies_only() {
    let site = parse(
        "name = \"foh\"\nnic_guid = \"{G}\"\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*LsoV2IPv4\"\nstring = \"0\"\nmust = true\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*EEE\"\nstring = \"0\"\n\
         [[registry]]\nkey = 'HKLM\\A'\nname = \"X\"\ndword = 1\n",
    )
    .unwrap();
    assert!(site.registry.iter().all(|e| e.nic_service.is_none()));
    let gone = Pc {
        nic_gone: true,
        nic_service: None,
        ..Pc::default()
    };
    let t = texts(&check(&site, &gone));
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(t[0].starts_with("must-have: the Dante NIC {G}"), "{t:?}");
    assert!(t[1].starts_with("registry HKLM\\A\\X: absent"), "{t:?}");
    assert_eq!(
        plan(&site, &gone),
        vec![Action::Reg {
            key: "HKLM\\A".into(),
            name: "X".into(),
            value: Value::Dword(1),
        }]
    );
    // Without a must-have on the NIC's keys the line is a plain one.
    let plain = parse(
        "name = \"foh\"\nnic_guid = \"{G}\"\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*EEE\"\nstring = \"0\"\n",
    )
    .unwrap();
    let found = check(&plain, &gone);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].what.starts_with("the Dante NIC {G}") && !found[0].must);
    // The NIC there, its driver read or not: the entries are read.
    let here = Pc::default();
    assert_eq!(texts(&check(&plain, &here)).len(), 1);
    assert!(texts(&check(&plain, &here))[0].starts_with("registry {nic_class}\\*EEE: absent"));
}

/// A Dante NIC found whose driver service is unreadable: the line names the
/// driver-scoped entries it leaves unjudged, must-have only through a
/// driver-scoped must-have, and a driver-free must-have on its keys is read
/// and judged on its own line (FOH after its RSS rollout, dasio#24).
#[test]
fn a_found_nic_with_an_unknown_driver_is_must_have_only_through_its_driver_entries() {
    let site = parse(
        "name = \"foh\"\nnic_guid = \"{G}\"\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*LsoV2IPv4\"\nstring = \"0\"\nmust = true\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*RssBaseProcNumber\"\nstring = \"1\"\nnic_service = \"rt640x64\"\n",
    )
    .unwrap();
    let mut here = Pc::default();
    here.reg.insert(
        ("{nic_class}".into(), "*LsoV2IPv4".into()),
        Value::String("1".into()),
    );
    let found = check(&site, &here);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].what, "the Dante NIC {G}");
    assert!(!found[0].must, "{found:?}");
    // The driver-free must-have is read and judged on its own line.
    assert_eq!(found[1].what, "registry {nic_class}\\*LsoV2IPv4");
    assert!(found[1].must, "{found:?}");
    assert_eq!(
        plan(&site, &here),
        vec![Action::Reg {
            key: "{nic_class}".into(),
            name: "*LsoV2IPv4".into(),
            value: Value::String("0".into()),
        }]
    );
    // A driver-scoped must-have it cannot judge makes the line one.
    let musts = parse(
        "name = \"foh\"\nnic_guid = \"{G}\"\n\
         [[registry]]\nkey = '{nic_class}'\nname = \"*RSS\"\nstring = \"1\"\nnic_service = \"rt640x64\"\nmust = true\n",
    )
    .unwrap();
    let found = check(&musts, &here);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].must, "{found:?}");
}

/// The service's loops (Windows glue, not compiled here) end the device on
/// a must-have problem, and the start checks before the device starts.
#[test]
fn the_service_stops_on_a_must_have_item() {
    let src = |f: &str| {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f)).unwrap()
    };
    let run = src("src/run.rs");
    let gate = run
        .find("profile::must_have_at_start()?")
        .expect("the start gate");
    let bridge = run
        .find("crate::run_bridge::run(")
        .expect("the bridge dispatch");
    assert!(gate < bridge, "the gate runs before both kinds start");
    for f in ["src/run.rs", "src/run_bridge.rs"] {
        let s = src(f);
        assert_eq!(
            s.matches("let Some(e) = profile.must_have()").count(),
            1,
            "{f} ends the device on a must-have problem once"
        );
    }
    let watch = src("src/run/profile.rs");
    assert!(watch.contains("crate::pc_profile::must_have_failure(p)"));
    // The start reads the whole profile (test signing is a bcd entry) and
    // refuses one that does not load; the 10-min in-process check skips
    // the entries that start a process.
    let gate = watch.find("fn must_have_at_start").expect("the start gate");
    assert!(watch[gate..].contains("in_process_problems(true)"));
    assert!(watch[gate..].contains("ChildCheck::NoProfile(e) => Some("));
    assert!(watch.contains("Ok(in_process_problems(false))"));
}

/// install.ps1 (PowerShell, run on the PCs only): the baseline step runs
/// from the bundle before the instance's service is stopped, refuses a
/// must-have item left unset (exit 2) and a site nic_guid that is not the
/// device's adapter.
#[test]
fn install_ps1_refuses_before_it_touches_the_instance() {
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
    let none = at("if (-not $profileGuid) {");
    let guid = at("if ($profileGuid -ne $adapterGuid) {");
    let apply = at("& (Join-Path $here \"dasio.exe\") pc-profile apply");
    let refuse = at("if ($applied -eq 2) {");
    let stop = at("Stop-Service -Name $service");
    let protect = at("Protect-DataFolder $root\n");
    assert!(protect < none, "the profile it reads is admin-only first");
    assert!(none < guid && guid < apply && apply < refuse && refuse < stop);
    assert!(s[none..guid].contains("throw \"the baseline step:"));
    // gpupdate's burst comes with the instance stopped.
    assert!(at("& gpupdate.exe") > stop);
    assert!(s[refuse..stop].contains("throw \"the baseline step:"));
    assert!(s[guid..apply].contains("throw \"the baseline step:"));
    // Applied before the stop, and again after the forced Group Policy
    // refresh: on mbc (18.11.2026) that refresh removed a policy value the
    // first apply had just written (Defender's DisableAntiSpyware, dasio#24).
    assert_eq!(
        s.matches(" pc-profile apply\n").count(),
        2,
        "applied before the stop and again after gpupdate"
    );
    let gp = at("& gpupdate.exe");
    let again = s[gp..]
        .find(" pc-profile apply\n")
        .map(|i| gp + i)
        .expect("applied again after gpupdate");
    assert!(apply < stop && stop < gp && gp < again);
    let after = &s[again..];
    let read = after
        .find("$applied = $LASTEXITCODE")
        .expect("the second apply's result is read");
    assert!(read < 80, "read right after the second apply");
    // A must-have item the refresh unset refuses the install like the first
    // apply does (the service would end on it anyway), before register.
    let refuse2 = after
        .find("if ($applied -eq 2) {")
        .expect("the second apply's exit 2 is handled");
    let register = after.find("register --instance").expect("register follows");
    assert!(read < refuse2 && refuse2 < register);
    assert!(after[refuse2..register].contains("throw \"the baseline step"));
}

/// install.ps1's `-NicDriver` step (dasio#128): opt-in only, the package
/// checked by dasio (whose refusal throws) before pnputil runs, the version
/// read back, all before the baseline step sets the NIC's settings again.
#[test]
fn install_ps1_installs_the_nic_driver_only_when_asked_and_only_a_checked_package() {
    let s = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install.ps1"),
    )
    .unwrap()
    .replace("\r\n", "\n");
    let at = |needle: &str| {
        s.find(needle)
            .unwrap_or_else(|| panic!("install.ps1 has no {needle}"))
    };
    let param = at("[string]$NicDriver = \"\"");
    let guid = at("if ($profileGuid -ne $adapterGuid) {");
    let opt_in = at("if ($NicDriver) {");
    let inside = at("StartsWith($bundleDir");
    let admin = at("Assert-AdminOnly $pkgDir");
    let verify = at("& (Join-Path $here \"dasio.exe\") @verifyArgs");
    let refused = at("if ($verified -ne 0) {");
    let pnputil = at("& pnputil.exe /add-driver");
    let back = at("if ($nowVersion -ne $wantVersion) {");
    let apply = at("& (Join-Path $here \"dasio.exe\") pc-profile apply");
    assert!(param < guid);
    assert!(
        guid < opt_in
            && opt_in < inside
            && inside < admin
            && admin < verify
            && verify < refused
            && refused < pnputil
            && pnputil < back
            && back < apply
    );
    assert!(s[refused..pnputil].contains("throw \"the NIC driver step: dasio refused"));
    assert!(s[back..apply].contains("throw \"the NIC driver step:"));
    assert!(s[opt_in..verify].contains("$verifyArgs = @(\"pc-profile\", \"nic-driver\")"));
    assert!(s[opt_in..verify].contains("\"--signature\", \"$($sig.Status)\""));
    assert_eq!(
        s.matches("pnputil.exe /add-driver").count(),
        1,
        "only in the opt-in step"
    );
}

/// The process keeper (dasio#185): install.ps1 turns its task off, ends
/// the keeper through its stop file (killing it only when it does not
/// end), copies dasio.exe as dasio-keep.exe (a few tries), registers the
/// SYSTEM task running `pc-profile keep` at boot and every 5 min (one at a
/// time: a dead keeper comes back), and in a `finally` removes the stop
/// file, turns the task on and starts it; then refuses when the keeper
/// does not run or ends at once. uninstall.ps1 removes it with the last
/// instance, in order.
#[test]
fn install_ps1_restarts_the_process_keeper_around_its_copy() {
    let read = |f: &str| {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f))
            .unwrap()
            .replace("\r\n", "\n")
    };
    let s = read("../../scripts/install.ps1");
    let at = |needle: &str| {
        s.find(needle)
            .unwrap_or_else(|| panic!("install.ps1 has no {needle}"))
    };
    let disable = at("Disable-ScheduledTask -TaskName $keepTask | Out-Null");
    let stop = at("Set-Content -Path $keepStop -Value \"install\"");
    let kill =
        at("Get-Process -Name \"dasio-keep\" -ErrorAction SilentlyContinue | Stop-Process -Force");
    let copy = at(
        "try { Copy-Item (Join-Path $here \"dasio.exe\") $keepExe -Force -ErrorAction Stop; break }",
    );
    let register = at("Register-ScheduledTask -TaskName $keepTask");
    let finally = at(
        "} finally {\n    if (Test-Path $keepStop) { Remove-Item $keepStop }\n    if (Get-ScheduledTask -TaskName $keepTask -ErrorAction SilentlyContinue) {\n        Enable-ScheduledTask -TaskName $keepTask | Out-Null\n        Start-ScheduledTask -TaskName $keepTask",
    );
    let start = at("throw \"the process keeper ($keepTask) did not start\"");
    let ended = at("throw \"the process keeper ($keepTask) ended right after its start");
    assert!(
        disable < stop
            && stop < kill
            && kill < copy
            && copy < register
            && register < finally
            && finally < start
            && start < ended
    );
    assert!(s.contains("$keepStop = Join-Path $env:ProgramData \"dasio\\pc-keep.stop\""));
    assert!(s.contains("New-ScheduledTaskAction -Execute $keepExe -Argument \"pc-profile keep\""));
    assert!(s.contains("(New-ScheduledTaskTrigger -AtStartup)"));
    assert!(s.contains("-RepetitionInterval (New-TimeSpan -Minutes 5)"));
    assert!(s.contains("-MultipleInstances IgnoreNew"));
    assert!(s.contains("-User SYSTEM -RunLevel Highest"));
    // Before the instance's own service is (re)made.
    assert!(ended < at("New-Service -Name $service"));
    let u = read("../../scripts/uninstall.ps1");
    let uat = |needle: &str| {
        u.find(needle)
            .unwrap_or_else(|| panic!("uninstall.ps1 has no {needle}"))
    };
    let off = uat("Disable-ScheduledTask -TaskName \"dasio-pc-keep\" | Out-Null");
    let stopped = uat("Set-Content -Path $keepStop -Value \"uninstall\"");
    let not_ended =
        uat("throw \"the process keeper did not end within 15 s; its task stays disabled");
    let gone = uat("Unregister-ScheduledTask -TaskName \"dasio-pc-keep\" -Confirm:$false");
    let exe = uat("throw \"removing dasio-keep.exe failed 10 times");
    let unstop = uat("    Remove-Item $keepStop\n}");
    assert!(off < stopped && stopped < not_ended && not_ended < gone && gone < exe && exe < unstop);
}
