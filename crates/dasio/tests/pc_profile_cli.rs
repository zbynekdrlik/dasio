//! `dasio pc-profile` as install.ps1 and the owner run it, on a
//! GitHub-hosted Windows runner (dasio#128): a must-have item missing
//! exits 2 with a `MUST-HAVE` line, and `layout` prints the CPU layout the
//! PC's count gives, and devices are read and disabled through the
//! configuration manager (dasio#181). `check` sets nothing; one test
//! disables the runner's kernel-debug NIC.
#![cfg(windows)]

use std::process::Command;

use dasio::pc_profile::Layout;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_dasio")
}

/// A site profile in a fresh folder, removed by the caller.
fn profile(tag: &str, text: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("dasio-cli-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pc-profile.toml");
    std::fs::write(&file, text).unwrap();
    (dir, file)
}

#[test]
fn check_exits_2_naming_a_missing_dante_nic_when_the_baseline_needs_its_settings() {
    // No adapter has this GUID: the baseline's must-have NIC settings
    // cannot be read, so the device must not run.
    let (dir, file) = profile(
        "must",
        "name = \"ci\"\nnic_guid = \"{00000000-0000-0000-0000-0000000DA510}\"\n",
    );
    let out = Command::new(exe())
        .args(["pc-profile", "check", "--profile"])
        .arg(&file)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(
        stdout.lines().any(|l| l.starts_with(
            "PROBLEM must-have: the Dante NIC {00000000-0000-0000-0000-0000000DA510}"
        )),
        "{stdout}"
    );
    let must = stdout
        .lines()
        .find(|l| l.starts_with("MUST-HAVE "))
        .unwrap_or_else(|| panic!("no MUST-HAVE line: {stdout}"));
    assert!(
        must.contains("must-have item(s) of dasio's baseline"),
        "{must}"
    );
    assert!(must.contains("the Dante NIC {00000000-"), "{must}");
}

#[test]
fn layout_prints_the_layout_of_the_pc_s_cpu_count() {
    let (dir, file) = profile("layout", "name = \"ci\"\n");
    let out = Command::new(exe())
        .args(["pc-profile", "layout", "--profile"])
        .arg(&file)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    // The runner's process may use all its CPUs: its count is the PC's.
    let cpus = std::thread::available_parallelism().unwrap().get();
    assert_eq!(
        stdout.trim_end(),
        format!(
            "{cpus} CPUs, 1 dasio instance(s): {}",
            Layout::of(cpus, 1).describe()
        )
    );
}

#[test]
fn an_existing_absent_exe_is_a_problem_and_apply_plan_names_its_uninstaller() {
    // dasio#128: the real reader on Windows. `--plan` runs nothing.
    let dir = std::env::temp_dir().join(format!("dasio-cli-absent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let there = dir.join("there.exe");
    std::fs::write(&there, b"x").unwrap();
    let gone = dir.join("gone.exe");
    let file = dir.join("pc-profile.toml");
    let entry = |exe: &std::path::Path| {
        format!(
            "[[absent]]\nexe = '{}'\nuninstall = ['C:\\Windows\\System32\\cmd.exe', '/c', 'exit']\n",
            exe.display()
        )
    };
    std::fs::write(
        &file,
        format!("name = \"ci\"\n{}{}", entry(&there), entry(&gone)),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(exe())
            .args(args)
            .arg("--profile")
            .arg(&file)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (code, check) = run(&["pc-profile", "check"]);
    let (plan_code, plan) = run(&["pc-profile", "apply", "--plan"]);
    let still = there.exists();
    std::fs::remove_dir_all(&dir).unwrap();
    // The runner misses the baseline's test signing too (exit 2): only
    // "not as it should be" counts here.
    assert_ne!(code, Some(0), "{check}");
    let line = format!("PROBLEM absent exe {}: present", there.display());
    assert!(check.lines().any(|l| l.starts_with(&line)), "{check}");
    assert!(!check.contains("gone.exe"), "{check}");
    assert_eq!(plan_code, Some(0), "{plan}");
    let would = format!(
        "would set the absence of {} (runs C:\\Windows\\System32\\cmd.exe /c exit)",
        there.display()
    );
    assert!(plan.lines().any(|l| l.starts_with(&would)), "{plan}");
    assert!(!plan.contains("gone.exe"), "{plan}");
    assert!(still, "--plan removed nothing");
}

#[test]
fn a_process_s_integrity_is_read_from_its_token() {
    // dasio#128: the real token read, on the check's own process (the
    // built dasio.exe, running while it checks). The runner's account runs
    // it at medium or high; exactly one of the two wants is met, and the
    // other is a problem naming the level read, never "unreadable".
    let run = |want: &str| {
        let (dir, file) = profile(
            &format!("integrity-{want}"),
            &format!("name = \"ci\"\n[[process]]\nexe = \"dasio.exe\"\nintegrity = \"{want}\"\n"),
        );
        let out = Command::new(exe())
            .args(["pc-profile", "check", "--profile"])
            .arg(&file)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        stdout
            .lines()
            .find(|l| l.starts_with("PROBLEM process dasio.exe integrity: "))
            .map(str::to_string)
    };
    let (medium, high) = (run("medium"), run("high"));
    match (medium, high) {
        (None, Some(line)) => assert!(
            line.starts_with(
                "PROBLEM process dasio.exe integrity: medium (the profile wants high)"
            ),
            "{line}"
        ),
        (Some(line), None) => assert!(
            line.starts_with(
                "PROBLEM process dasio.exe integrity: high (the profile wants medium)"
            ),
            "{line}"
        ),
        other => panic!("exactly one of medium/high must hold: {other:?}"),
    }
}

/// The tests that change a device run only on a GitHub-hosted runner
/// (thrown away after the job): on a dasio PC they would hold a real
/// device off. A self-hosted runner sets GITHUB_ACTIONS too.
fn on_the_runner() {
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "this test disables a device: run it on a GitHub runner only"
    );
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted"),
        "this test disables a device: run it on a GitHub-hosted runner only"
    );
}

/// The runner's kernel-debug NIC: a network adapter that
/// `apply_disables_a_device_now_and_for_every_boot` disables while the
/// other tests run, so no other test picks or counts it.
const KDNIC: &str = r"ROOT\KDNIC\0000";

/// The device tree's root.
const ROOT: &str = r"HTREE\ROOT\0";

/// The trimmed, non-empty lines PowerShell prints for `cmd` (its error
/// output goes to the test's log: a read that skips a device it cannot
/// read still exits 1, so the callers judge the lines).
fn ps(cmd: &str) -> Vec<String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", cmd])
        .output()
        .unwrap();
    eprintln!("{cmd}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// `id` inside a single-quoted PowerShell string.
fn quoted(id: &str) -> String {
    format!("'{}'", id.replace('\'', "''"))
}

/// Instance paths upper case, sorted, each once (Windows ignores case).
fn id_set<S: AsRef<str>>(ids: &[S]) -> Vec<String> {
    let mut v: Vec<String> = ids
        .iter()
        .map(|s| s.as_ref().to_ascii_uppercase())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn device_entry(id: &str, name: &str) -> String {
    format!("[[device]]\ninstance_id = '{id}'\nname = \"{name}\"\nwhy = \"ci\"\n")
}

/// The ROOT-enumerated devices of the runner that run without a problem,
/// as PowerShell's PnpDevice module reads them.
fn running_root_devices() -> Vec<String> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-PnpDevice -PresentOnly -Status OK | Where-Object { $_.InstanceId -like 'ROOT\\*' } | ForEach-Object { $_.InstanceId }",
        ])
        .output()
        .unwrap();
    let ids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|id| id.split('\\').count() >= 3)
        .map(String::from)
        .collect();
    assert!(
        !ids.is_empty(),
        "no running ROOT device on the runner: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    ids
}

/// An adapter of the runner that is up and has a device (a virtual one
/// without a PnP device, which a runner image can list first, has no
/// device path): its NetCfgInstanceId (the profile's `nic_guid`) and its
/// device instance path.
fn runner_nic() -> (String, String) {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "$a = Get-NetAdapter | Where-Object { $_.Status -eq 'Up' -and $_.PnPDeviceID } | Sort-Object ifIndex | Select-Object -First 1; $a.InterfaceGuid; $a.PnPDeviceID",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    match (lines.next(), lines.next()) {
        (Some(guid), Some(pnp)) => (guid.to_string(), pnp.to_string()),
        _ => panic!("no adapter that is up on the runner: {text:?}"),
    }
}

#[test]
fn check_reads_a_device_through_the_configuration_manager_and_plan_names_the_disable() {
    // dasio#181: the real reader, with the runner's adapter as the Dante
    // NIC (without its device path nothing is checked). Read only: --plan
    // disables nothing.
    use dasio::pc_profile::{Action, Devices, parse, plan, windows::Pc};
    let (guid, _) = runner_nic();
    let missing = r"ROOT\DASIO_CI_NONE\0000";
    let entry = device_entry;
    let head = format!("name = \"ci\"\nnic_guid = \"{guid}\"\n");
    // A device the guard leaves free (dasio#181 refuses every one that can
    // cut a network path), as the library plans it; never KDNIC, which
    // another test disables meanwhile.
    let candidates: String = running_root_devices()
        .iter()
        .filter(|id| !id.eq_ignore_ascii_case(KDNIC))
        .enumerate()
        .map(|(i, id)| entry(id, &format!("c{i}")))
        .collect();
    let probe_profile = parse(&format!("{head}{candidates}")).unwrap();
    let probe = Pc::new(&probe_profile);
    assert!(
        !probe.nic_chain().is_empty(),
        "the runner's adapter {guid}: no device path"
    );
    let running = plan(&probe_profile, &probe)
        .into_iter()
        .find_map(|a| match a {
            Action::Disable(id) => Some(id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the guard frees no running ROOT device: {candidates}"));
    let (dir, file) = profile(
        "device",
        &format!(
            "{head}{}{}",
            entry(&running, "running"),
            entry(missing, "missing")
        ),
    );
    let run = |args: &[&str]| {
        let out = Command::new(exe())
            .args(args)
            .arg("--profile")
            .arg(&file)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let check = run(&["pc-profile", "check"]);
    let plan = run(&["pc-profile", "apply", "--plan"]);
    std::fs::remove_dir_all(&dir).unwrap();
    let line = format!("PROBLEM device running ({running}): on (the profile wants off) — ci");
    assert!(check.lines().any(|l| l == line), "{check}");
    let line =
        format!("PROBLEM device missing ({missing}): not on this PC (the profile wants off) — ci");
    assert!(check.lines().any(|l| l == line), "{check}");
    let would = format!(
        "would set device {running} off (takes effect after nothing more (the device stops now; ConfigFlags keeps it off at every boot))"
    );
    assert!(plan.lines().any(|l| l == would), "{plan}");
    assert!(!plan.contains("DASIO_CI_NONE"), "{plan}");
}

#[test]
fn apply_disables_a_device_now_and_for_every_boot() {
    on_the_runner();
    // dasio#181: the real disable, on the runner's kernel-debug NIC (no
    // job of the runner uses it). Through the library, not `pc-profile
    // apply`: that would apply the whole baseline to the runner.
    use dasio::pc_profile::{Action, DevState, Devices, parse, windows::Pc};
    let id = r"ROOT\KDNIC\0000";
    let pc = Pc::new(&parse("name = \"ci\"\n").unwrap());
    let before = pc
        .device(id)
        .unwrap()
        .unwrap_or_else(|| panic!("the runner has no {id}"));
    assert!(before.present && !before.off(), "{id} before: {before:?}");
    pc.apply(&Action::Disable(id.into())).unwrap();
    let off = DevState {
        present: true,
        problem: Some(22),
        config_disabled: true,
    };
    assert_eq!(pc.device(id).unwrap(), Some(off));
    // Again: nothing left to do, still off.
    pc.apply(&Action::Disable(id.into())).unwrap();
    assert_eq!(pc.device(id).unwrap(), Some(off));
    // A device the PC does not have is an error naming it.
    assert_eq!(
        pc.apply(&Action::Disable(r"ROOT\DASIO_CI_NONE\0000".into())),
        Err(r"device ROOT\DASIO_CI_NONE\0000: not on this PC".into())
    );
}

#[test]
fn apply_holds_an_unplugged_device_off_for_when_it_comes_back() {
    on_the_runner();
    // dasio#181: a device known but not connected (a phantom devnode, like
    // mbc's Wi-Fi) gets CONFIGFLAG_DISABLED, so it stays off when it comes
    // back. On a phantom of the runner that is not held off yet.
    use dasio::pc_profile::{Action, DevState, Devices, parse, windows::Pc};
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-PnpDevice | Where-Object { -not $_.Present } | ForEach-Object { $_.InstanceId }",
        ])
        .output()
        .unwrap();
    let ids = String::from_utf8_lossy(&out.stdout).into_owned();
    let pc = Pc::new(&parse("name = \"ci\"\n").unwrap());
    let id = ids
        .lines()
        .map(str::trim)
        .find(|id| {
            matches!(
                pc.device(id),
                Ok(Some(DevState {
                    present: false,
                    config_disabled: false,
                    ..
                }))
            )
        })
        .unwrap_or_else(|| panic!("no unplugged device on the runner that is not held off: {ids}"))
        .to_string();
    pc.apply(&Action::Disable(id.clone())).unwrap();
    assert_eq!(
        pc.device(&id).unwrap(),
        Some(DevState {
            present: false,
            problem: None,
            config_disabled: true,
        }),
        "{id}"
    );
}

#[test]
fn the_dante_nic_s_parent_chain_is_read_up_to_the_root() {
    // dasio#181: the [[device]] guard's chain on a real adapter of the
    // runner, read as Pc::new reads it from the profile's nic_guid.
    use dasio::pc_profile::{Devices, parse, windows::Pc};
    let (guid, pnp) = runner_nic();
    let entry =
        "[[device]]\ninstance_id = 'ROOT\\DASIO_CI_NONE\\0000'\nname = \"x\"\nwhy = \"ci\"\n";
    let pc = Pc::new(&parse(&format!("name = \"ci\"\nnic_guid = \"{guid}\"\n{entry}")).unwrap());
    let chain = pc.nic_chain();
    assert!(chain.len() >= 2, "{chain:?}");
    assert!(chain[0].eq_ignore_ascii_case(&pnp), "{chain:?} vs {pnp}");
    assert_eq!(
        chain.last().map(String::as_str),
        Some(r"HTREE\ROOT\0"),
        "{chain:?}"
    );
    // Without a [[device]] entry the chain is not walked.
    let none = Pc::new(&parse(&format!("name = \"ci\"\nnic_guid = \"{guid}\"\n")).unwrap());
    assert_eq!(none.nic_chain(), Vec::<String>::new());
}

#[test]
fn every_present_network_adapter_is_read_with_its_parents() {
    // dasio#181: the guard's adapters are every present Net-class device
    // (class + present filter), as Windows' own PnP module lists them,
    // whatever their address or link; each with its parents up to the
    // root, as Windows gives each device's parent.
    use dasio::pc_profile::{Devices, parse, windows::Pc};
    let pc = Pc::new(&parse("name = \"ci\"\n").unwrap());
    let adapters = pc.net_adapters().unwrap();
    let not_kdnic = |ids: Vec<String>| -> Vec<String> {
        id_set(&ids)
            .into_iter()
            .filter(|id| id.as_str() != KDNIC)
            .collect()
    };
    assert_eq!(
        not_kdnic(adapters.iter().map(|a| a.id.clone()).collect()),
        not_kdnic(ps(
            "Get-PnpDevice -PresentOnly -Class Net | ForEach-Object { $_.InstanceId }"
        )),
        "{adapters:?}"
    );
    for a in &adapters {
        assert_eq!(a.parents.last().map(String::as_str), Some(ROOT), "{a:?}");
    }
    // The runner's adapter that is up: its parents, nearest first, as
    // Windows gives each one's DEVPKEY_Device_Parent up to the root.
    let (_, pnp) = runner_nic();
    let nic = adapters
        .iter()
        .find(|a| a.id.eq_ignore_ascii_case(&pnp))
        .unwrap_or_else(|| panic!("{pnp} is not among {adapters:?}"));
    let windows = ps(&format!(
        "$id = {}; foreach ($i in 1..64) {{ $id = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_Parent).Data; if (-not $id) {{ break }}; $id }}",
        quoted(&pnp)
    ));
    let upper =
        |v: &[String]| -> Vec<String> { v.iter().map(|s| s.to_ascii_uppercase()).collect() };
    assert_eq!(upper(&nic.parents), upper(&windows), "{nic:?}");
    assert_eq!(pc.parents(&pnp).unwrap(), nic.parents);
}

#[test]
fn what_stops_with_a_device_is_its_children_and_its_removal_relations() {
    // dasio#181: disabling a device removes its children and its removal
    // relations; the guard's read of them against Windows' own properties,
    // on a device that has removal relations (a disk: its volumes).
    use dasio::pc_profile::{Devices, parse, windows::Pc};
    let pc = Pc::new(&parse("name = \"ci\"\n").unwrap());
    let lines = ps(
        "Get-PnpDevice -PresentOnly | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_RemovalRelations -ErrorAction SilentlyContinue | Where-Object { $_.Data } | Select-Object -First 1 | ForEach-Object { $_.InstanceId; $_.Data }",
    );
    let (id, relations) = lines
        .split_first()
        .expect("no device with removal relations on the runner");
    let children = |id: &str| {
        ps(&format!(
            "(Get-PnpDeviceProperty -InstanceId {} -KeyName DEVPKEY_Device_Children).Data",
            quoted(id)
        ))
    };
    let mut want = children(id.as_str());
    want.extend(relations.iter().cloned());
    assert_eq!(id_set(&pc.removes(id).unwrap()), id_set(&want), "{id}");
    // The runner adapter's parent: the adapter is among its children.
    let (_, pnp) = runner_nic();
    let parent = pc.parents(&pnp).unwrap()[0].clone();
    let stops = pc.removes(&parent).unwrap();
    assert!(
        stops.iter().any(|c| c.eq_ignore_ascii_case(&pnp)),
        "{parent}: {stops:?}"
    );
    // A node Windows does not know at all is unread, never one that stops
    // nothing (review of PR #196: the guard then fails closed).
    assert_eq!(
        pc.removes(r"ROOT\DASIO_CI_NONE\0000"),
        Err(r"ROOT\DASIO_CI_NONE\0000: no such device node".to_string())
    );
}

#[test]
fn check_refuses_a_network_adapter_and_a_device_the_dante_nic_hangs_on() {
    // dasio#181: through the exe, read only (--plan disables nothing): an
    // adapter of the runner that is not the Dante NIC (no matter its
    // address) and the device the Dante NIC hangs on are refused, never
    // planned. Never KDNIC: another test disables it meanwhile.
    use dasio::pc_profile::{Devices, parse, windows::Pc};
    let (guid, _) = runner_nic();
    let head = format!("name = \"ci\"\nnic_guid = \"{guid}\"\n");
    let missing = r"ROOT\DASIO_CI_NONE\0000";
    let probe = Pc::new(&parse(&format!("{head}{}", device_entry(missing, "m"))).unwrap());
    let chain = probe.nic_chain();
    assert!(chain.len() >= 2, "{chain:?}");
    let hub = chain[1].clone();
    let adapters = probe.net_adapters().unwrap();
    let other = adapters
        .iter()
        .map(|a| a.id.clone())
        .find(|id| {
            !chain.iter().any(|c| c.eq_ignore_ascii_case(id))
                && !id.eq_ignore_ascii_case(KDNIC)
                && matches!(probe.device(id), Ok(Some(s)) if !s.off())
        })
        .unwrap_or_else(|| panic!("no second adapter that is on: {adapters:?}"));
    let (dir, file) = profile(
        "guard",
        &format!(
            "{head}{}{}",
            device_entry(&other, "other adapter"),
            device_entry(&hub, "nic parent")
        ),
    );
    let run = |args: &[&str]| {
        let out = Command::new(exe())
            .args(args)
            .arg("--profile")
            .arg(&file)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let check = run(&["pc-profile", "check"]);
    let plan = run(&["pc-profile", "apply", "--plan"]);
    std::fs::remove_dir_all(&dir).unwrap();
    let refused = "(the profile wants an entry for another device) — never disabled by the profile: it cuts the PC off (dasio#181)";
    for line in [
        format!("PROBLEM device other adapter ({other}): a network adapter {refused}"),
        format!("PROBLEM device nic parent ({hub}): a device the Dante NIC hangs on {refused}"),
    ] {
        assert!(check.lines().any(|l| l == line), "{line}\n{check}");
    }
    for id in [&other, &hub] {
        assert!(!plan.contains(&format!("device {id} off")), "{id}: {plan}");
    }
}

#[test]
fn the_keeper_gives_a_running_process_its_entry_s_priority_and_affinity() {
    // dasio#185: one pass of `dasio pc-profile keep` over a real process
    // of the runner (a waiting ping, started here), read back.
    use dasio::pc_profile::{Reader, keep::Fights, parse, windows::Pc, windows::keep::pass};
    let mut child = Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let p = parse(
        "name = \"ci\"\n[[process]]\nexe = \"ping.exe\"\naffinity = \"0x1\"\npriority = \"below normal\"\n",
    )
    .unwrap();
    let pc = Pc::new(&p);
    let before = pc.process(&p.process[0]);
    let mut fights = Fights::default();
    let mut failed = std::collections::HashSet::new();
    let lines = pass(&p, &mut fights, &mut failed, 0);
    let after = pc.process(&p.process[0]);
    let again = pass(&p, &mut fights, &mut failed, 2_000);
    child.kill().unwrap();
    child.wait().unwrap();
    let before = before.expect("the ping runs");
    let after = after.expect("the ping still runs");
    assert_eq!(
        (after.affinity, after.priority.as_str()),
        (0x1, "below normal")
    );
    let changed = format!("kept ping.exe (pid {}): ", child.id());
    assert!(
        lines.iter().any(|l| l.starts_with(&changed)),
        "{lines:?} (before: {before:?})"
    );
    // Held already: the second pass sets and logs nothing.
    assert_eq!(again, Vec::<String>::new());
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
fn a_value_in_system_s_own_hive_is_set_and_read_back_there() {
    on_the_runner();
    // dasio#191: dsg's SoundGrid ASIO buffer lives in the hive of the user
    // that runs the ASIO host, SYSTEM's for dasio. Through the library, not
    // `pc-profile apply`: that would apply the whole baseline to the runner.
    use dasio::pc_profile::{Action, Reader, Value, parse, windows::Pc};
    let sub = format!(r"Software\dasio-ci-{}", std::process::id());
    let key = format!(r"HKU\S-1-5-18\{sub}");
    // Removed also when an assert below fails.
    struct Remove(String);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = Command::new("reg").args(["delete", &self.0, "/f"]).status();
        }
    }
    let _remove = Remove(key.clone());
    let pc = Pc::new(&parse("name = \"ci\"\n").unwrap());
    assert_eq!(pc.reg(&key, "Buffer Size(Samples)"), Ok(None));
    let value = Value::Binary(vec![0x80, 0, 0, 0]);
    pc.apply(&Action::Reg {
        key: key.clone(),
        name: "Buffer Size(Samples)".into(),
        value: value.clone(),
    })
    .unwrap();
    assert_eq!(pc.reg(&key, "Buffer Size(Samples)"), Ok(Some(value)));
    // In SYSTEM's hive, not HKLM's: reg.exe finds it there too.
    assert_eq!(
        pc.reg(&format!(r"HKLM\{sub}"), "Buffer Size(Samples)"),
        Ok(None)
    );
    let q = Command::new("reg")
        .args(["query", &key, "/v", "Buffer Size(Samples)"])
        .output()
        .unwrap();
    assert!(q.status.success(), "reg query {key}");
    assert!(
        String::from_utf8_lossy(&q.stdout).contains("REG_BINARY    80000000"),
        "{}",
        String::from_utf8_lossy(&q.stdout)
    );
    // Another user's hive is refused before anything is touched.
    assert!(
        pc.reg(r"HKU\S-1-5-21-1-2-3-1001\Software\X", "Y")
            .unwrap_err()
            .contains("only SYSTEM's own hive")
    );
}

/// Windows' own answer for `pid`'s GPU scheduling class, read apart from
/// dasio (PowerShell's P/Invoke, a handle with query + set rights):
/// `Ok(class)` or `Err(NTSTATUS)`.
fn gpu_class_by_powershell(pid: u32) -> Result<u32, String> {
    let script = format!(
        r##"Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class G {{ [DllImport("gdi32.dll")] public static extern int D3DKMTGetProcessSchedulingPriorityClass(IntPtr h, out int c); [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint a, bool i, uint p); }}'; $h = [G]::OpenProcess(0x600, $false, {pid}); $c = -1; $st = [G]::D3DKMTGetProcessSchedulingPriorityClass($h, [ref]$c); "$st $c""##
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (st, class) = text.split_once(' ').unwrap_or_else(|| panic!("{text:?}"));
    if st == "0" {
        Ok(class.parse().unwrap())
    } else {
        Err(format!("NTSTATUS {st}"))
    }
}

#[test]
fn the_keeper_gives_a_running_process_its_io_memory_and_gpu_priority_and_boost() {
    // dasio#185: what Process Lasso set on FOH beyond affinity and priority
    // class, set by one keeper pass on a real process of the runner (a
    // waiting ping, started here) and read back through the check's reader.
    use dasio::pc_profile::{
        Extras, GPU_PRIORITIES, Reader, check, keep::Fights, parse, windows::Pc,
        windows::keep::pass,
    };
    let mut child = Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let entry = |io: &str, memory: &str, gpu: &str, boost: bool| {
        parse(&format!(
            "name = \"ci\"\n[[process]]\nexe = \"ping.exe\"\nio_priority = \"{io}\"\nmemory_priority = \"{memory}\"\ngpu_priority = \"{gpu}\"\npriority_boost = {boost}\n"
        ))
        .unwrap()
    };
    // What the ping has now (it inherits the test's I/O and memory
    // priority: low on the runner), then targets that differ from it.
    let probe = entry("normal", "normal", "normal", true);
    let before = Pc::new(&probe)
        .process(&probe.process[0])
        .expect("the ping runs");
    // The GPU class as Windows itself answers it: a PC without a GPU
    // scheduler (the runner's VM) refuses the read, dsg's Intel GPU answers.
    // On the runner this proves the unreadable path; the set path is
    // proven on dsg after the deploy (dasio#185).
    let windows_gpu = gpu_class_by_powershell(child.id());
    assert_eq!(
        before.extra.gpu,
        windows_gpu
            .as_ref()
            .ok()
            .map(|c| GPU_PRIORITIES[*c as usize].to_string()),
        "dasio's read against Windows' own ({windows_gpu:?})"
    );
    let other = |now: &Option<String>, a: &'static str, b: &'static str| {
        if now.as_deref() == Some(a) { b } else { a }
    };
    let gpu_target = other(&before.extra.gpu, "below normal", "idle");
    let want = Extras {
        io: Some(other(&before.extra.io, "very low", "low").into()),
        memory: Some(other(&before.extra.memory, "very low", "low").into()),
        // Unreadable stays unreadable: the keeper sets nothing it did not
        // read.
        gpu: windows_gpu.is_ok().then(|| gpu_target.to_string()),
        boost: Some(!before.extra.boost.expect("its boost reads")),
    };
    let p = entry(
        want.io.as_deref().unwrap(),
        want.memory.as_deref().unwrap(),
        gpu_target,
        want.boost.unwrap(),
    );
    let pc = Pc::new(&p);
    let mut fights = Fights::default();
    let mut failed = std::collections::HashSet::new();
    let lines = pass(&p, &mut fights, &mut failed, 0);
    let after = pc.process(&p.process[0]);
    let problems = check(&p, &pc);
    let again = pass(&p, &mut fights, &mut failed, 2_000);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        failed.is_empty(),
        "{failed:?} {lines:?} (before: {before:?})"
    );
    let after = after.expect("the ping still runs");
    assert_eq!(after.extra, want, "before: {before:?}");
    let changed = format!(
        "kept ping.exe (pid {}): io priority {} -> {}",
        child.id(),
        before.extra.io.as_deref().unwrap_or("?"),
        want.io.as_deref().unwrap()
    );
    assert!(lines.iter().any(|l| l.starts_with(&changed)), "{lines:?}");
    assert_eq!(
        lines.iter().any(|l| l.contains("gpu priority")),
        windows_gpu.is_ok(),
        "{lines:?}"
    );
    // The check: held, except a GPU class Windows will not read.
    let rows: Vec<(String, String)> = problems
        .into_iter()
        .filter(|x| x.what.starts_with("process ping.exe"))
        .map(|x| (x.what, x.found))
        .collect();
    let unreadable = vec![(
        "process ping.exe gpu priority".to_string(),
        "unreadable".to_string(),
    )];
    assert_eq!(
        rows,
        if windows_gpu.is_ok() {
            Vec::new()
        } else {
            unreadable
        }
    );
    // Held already: the second pass sets and logs nothing.
    assert_eq!(again, Vec::<String>::new());
}

/// A PowerShell that waits a minute: a process of the runner that no other
/// test's keeper pass touches (those name ping.exe only).
fn sleeper() -> std::process::Child {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 60",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn the_keeper_gives_every_other_process_the_catch_all_unless_it_set_its_own() {
    // dasio#185: Process Lasso's `*` rule. Two processes of the runner,
    // started here: one on every CPU gets `others_affinity` in one pass,
    // one the test pinned itself is left alone, and a second pass sets and
    // logs nothing. The pass goes over these two only (`pass_over`): over
    // the whole list it would pin every process of the runner, the other
    // tests' too.
    use dasio::pc_profile::{
        Reader,
        keep::Fights,
        parse, resolve,
        windows::{Pc, cpu_count, keep::pass_over},
    };
    use std::os::windows::io::AsRawHandle;
    let cpus = cpu_count().unwrap();
    assert!(cpus >= 2, "the runner has {cpus} CPU(s)");
    let mut p = parse("name = \"ci\"\nothers_affinity = \"{system}\"\n").unwrap();
    resolve(&mut p, cpus).unwrap();
    let want = Layout::of(cpus, 1).system_mask();
    assert_eq!(p.others_affinity, Some(format!("0x{want:X}")));
    // The last CPU: never a system CPU (they come first), never every CPU.
    let pinned = 1u64 << (cpus - 1);
    let mut free = sleeper();
    let mut own = sleeper();
    // SAFETY: the child's own process handle (all access) and a mask of
    // one of the runner's CPUs.
    let ok = unsafe {
        windows_sys::Win32::System::Threading::SetProcessAffinityMask(
            own.as_raw_handle(),
            pinned as usize,
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    let pc = Pc::new(&p);
    let read = |pid: u32| {
        let all = pc.processes().expect("the process list reads");
        let me = all.list.iter().find(|s| s.pid == pid).map(|s| s.affinity);
        (me, all.system)
    };
    let (free_before, all) = read(free.id());
    let (own_before, _) = read(own.id());
    let procs = [
        (free.id(), "powershell.exe".to_string()),
        (own.id(), "powershell.exe".to_string()),
    ];
    let mut fights = Fights::default();
    let mut failed = std::collections::HashSet::new();
    let lines = pass_over(&p, &procs, &mut fights, &mut failed, 0);
    let again = pass_over(&p, &procs, &mut fights, &mut failed, 2_000);
    let (free_after, _) = read(free.id());
    let (own_after, _) = read(own.id());
    for c in [&mut free, &mut own] {
        c.kill().unwrap();
        c.wait().unwrap();
    }
    assert_eq!(
        free_before,
        Some(all),
        "a process started here runs on every CPU"
    );
    assert_eq!(own_before, Some(pinned));
    assert_eq!(
        lines,
        [format!(
            "kept powershell.exe (pid {}): affinity 0x{all:X} -> 0x{want:X} (others_affinity)",
            free.id()
        )]
    );
    assert_eq!(free_after, Some(want));
    assert_eq!(
        own_after,
        Some(pinned),
        "a process with its own affinity is left alone"
    );
    // Held already: the second pass sets and logs nothing.
    assert_eq!(again, Vec::<String>::new());
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
fn check_names_the_processes_the_catch_all_does_not_hold_in_one_line() {
    // dasio#185: the runner's processes run on every CPU (no keeper runs
    // there): `check` reads them with the keeper's own rights and says so
    // in ONE problem, never one per process.
    let (dir, file) = profile("others", "name = \"ci\"\nothers_affinity = \"{system}\"\n");
    let out = Command::new(exe())
        .args(["pc-profile", "check", "--profile"])
        .arg(&file)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let cpus = dasio::pc_profile::windows::cpu_count().unwrap();
    let want = Layout::of(cpus, 1).system_mask();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("PROBLEM others_affinity"))
        .collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(
        lines[0].starts_with("PROBLEM others_affinity: not held: "),
        "{text}"
    );
    assert!(
        lines[0].contains(&format!("(the profile wants 0x{want:X})")),
        "{text}"
    );
    assert_ne!(out.status.code(), Some(0), "{text}");
}
