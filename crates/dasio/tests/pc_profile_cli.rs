//! `dasio pc-profile` as install.ps1 and the owner run it, on a
//! GitHub-hosted Windows runner (dasio#128): a must-have item missing
//! exits 2 with a `MUST-HAVE` line, and `layout` prints the CPU layout the
//! PC's count gives. Read only: `check` sets nothing.
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
