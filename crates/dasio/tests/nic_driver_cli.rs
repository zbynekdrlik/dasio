//! `dasio pc-profile nic-driver` as install.ps1's `-NicDriver` step runs it
//! (dasio#128): the built exe, its exit codes and lines, `--profile` taken
//! out before the decision. Portable (no Windows call): runs on every CI
//! runner.

use std::process::Command;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_dasio")
}

const SIGNER: &str = "CN=Microsoft Windows Hardware Compatibility Publisher, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";

/// install.ps1's arguments for the baseline's pinned package on FOH's NIC,
/// with `change` applied to the file list.
fn args(change: impl Fn(&mut Vec<(String, String)>)) -> Vec<String> {
    let base = dasio::pc_profile::baseline().unwrap();
    let mut files: Vec<(String, String)> = base.nic_driver[0]
        .files
        .iter()
        .map(|f| (f.name.clone(), f.sha256.clone()))
        .collect();
    change(&mut files);
    let mut a: Vec<String> = ["pc-profile", "nic-driver", "--hardware-id"]
        .map(String::from)
        .to_vec();
    a.push(r"PCI\VEN_10EC&DEV_8125&SUBSYS_E0001458&REV_05".into());
    for (n, h) in files {
        a.push("--file".into());
        a.push(format!("{n}={h}"));
    }
    a.extend(["--signature", "Valid", "--signer", SIGNER].map(String::from));
    a
}

/// Runs it with `--profile <profile>` in the middle of the arguments.
fn run(args: &[String], profile: &std::path::Path) -> (Option<i32>, String) {
    let mut cmd = Command::new(exe());
    cmd.args(&args[..2])
        .arg("--profile")
        .arg(profile)
        .args(&args[2..]);
    let out = cmd.output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
    )
}

#[test]
fn nic_driver_accepts_the_pinned_package_and_refuses_any_other() {
    let dir = std::env::temp_dir().join(format!("dasio-nicdrv-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // No site profile there: the baseline alone.
    let none = dir.join("none.toml");
    let broken = dir.join("broken.toml");
    std::fs::write(&broken, "name = [").unwrap();
    let ok = run(&args(|_| {}), &none);
    let changed = run(&args(|f| f[2].1 = "00".repeat(32)), &none);
    let not_loading = run(&args(|_| {}), &broken);
    let bad = run(&args(|f| f[0].0.clear()), &none);
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(
        ok,
        (
            Some(0),
            "the package is Realtek PCIe (rt640x64)'s pinned driver (rt640x64sta.inf 10.79.20.1003, signed by Microsoft Windows Hardware Compatibility Publisher)\nINF rt640x64sta.inf\nVERSION 10.79.20.1003\n".to_string()
        )
    );
    assert_eq!(changed.0, Some(2), "{}", changed.1);
    assert!(
        changed.1.starts_with("REFUSED this is not Realtek PCIe (rt640x64)'s pinned package (rt640x64sta.inf 10.79.20.1003 from "),
        "{}",
        changed.1
    );
    assert!(
        changed.1.contains(&format!(
            "rt640x64.sys: SHA256 {}, the baseline pins ",
            "00".repeat(32)
        )),
        "{}",
        changed.1
    );
    assert_eq!(not_loading.0, Some(2), "{}", not_loading.1);
    assert!(
        not_loading
            .1
            .starts_with("REFUSED the PC profile does not load: "),
        "{}",
        not_loading.1
    );
    assert_eq!(bad.0, Some(64), "{}", bad.1);
    assert!(
        bad.1.starts_with("--file =") && bad.1.ends_with(": give <name>=<sha256>\n"),
        "{}",
        bad.1
    );
}
