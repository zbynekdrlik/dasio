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

/// The tests that change a device run only on a GitHub runner (thrown away
/// after the job): on a dasio PC they would hold a real device off.
fn on_the_runner() {
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "this test disables a device: run it on a GitHub runner only"
    );
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

/// An adapter of the runner that is up: its NetCfgInstanceId (the
/// profile's `nic_guid`) and its device instance path.
fn runner_nic() -> (String, String) {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "$a = Get-NetAdapter | Where-Object Status -eq 'Up' | Sort-Object ifIndex | Select-Object -First 1; $a.InterfaceGuid; $a.PnPDeviceID",
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
    use dasio::pc_profile::{Reader, parse, windows::Pc};
    let (guid, _) = runner_nic();
    let missing = r"ROOT\DASIO_CI_NONE\0000";
    let entry = |id: &str, name: &str| {
        format!("[[device]]\ninstance_id = '{id}'\nname = \"{name}\"\nwhy = \"ci\"\n")
    };
    let head = format!("name = \"ci\"\nnic_guid = \"{guid}\"\n");
    let chain =
        Pc::new(&parse(&format!("{head}{}", entry(missing, "missing"))).unwrap()).nic_chain();
    assert!(
        !chain.is_empty(),
        "the runner's adapter {guid}: no device path"
    );
    let running = running_root_devices()
        .into_iter()
        .find(|id| !chain.iter().any(|c| c.eq_ignore_ascii_case(id)))
        .unwrap_or_else(|| panic!("every running ROOT device is on the NIC's chain {chain:?}"));
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
    use dasio::pc_profile::{Action, DevState, Reader, parse, windows::Pc};
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
    use dasio::pc_profile::{Action, DevState, Reader, parse, windows::Pc};
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
    use dasio::pc_profile::{Reader, parse, windows::Pc};
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
fn the_keeper_gives_a_running_process_its_entry_s_priority_and_affinity() {
    // dasio#185: one pass of `dasio pc-profile keep` over a real process
    // of the runner (a waiting ping, started here), read back.
    use dasio::pc_profile::{Reader, keep::Quiet, parse, windows::Pc, windows::keep::pass};
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
    let before = pc.process("ping.exe");
    let mut quiet = Quiet::default();
    let mut failed = std::collections::HashSet::new();
    let lines = pass(&p, &mut quiet, &mut failed, 0);
    let after = pc.process("ping.exe");
    let again = pass(&p, &mut quiet, &mut failed, 2_000);
    child.kill().unwrap();
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
