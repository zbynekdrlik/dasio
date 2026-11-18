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
    assert!(
        b.registry
            .iter()
            .filter(|e| e.must)
            .all(|e| e.nic_service.is_some())
    );
    assert!(b.bcd.iter().any(|e| e.name == "testsigning" && e.must));
    assert!(
        !b.bcd
            .iter()
            .any(|e| e.name == "disabledynamictick" && e.must)
    );
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
