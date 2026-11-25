//! A written NIC value is not a running one (dasio#26).

use super::super::nic_restart::{
    NicTimes, SLACK_SECS, check as restart_check, filetime_secs, pending,
};
use super::*;

const NIC: &str = "name = \"b\"\nnic_guid = \"{G}\"\n[[registry]]\nkey = '{nic_class}'\nname = \"*EEE\"\nstring = \"0\"\n";

fn nic_pc(have: &str, keys_written: u64, started: u64) -> Pc {
    let mut pc = Pc::default();
    pc.reg.insert(
        ("{nic_class}".into(), "*EEE".into()),
        Value::String(have.into()),
    );
    pc.nic_times = Some(NicTimes {
        keys_written,
        started,
    });
    pc
}

fn profile(text: &str) -> Profile {
    parse_with(text, false).unwrap()
}

fn restart_lines(p: &Profile, pc: &Pc) -> Vec<String> {
    check(p, pc)
        .iter()
        .filter(|x| x.what.starts_with("the Dante NIC's registry"))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn keys_written_after_the_start_by_more_than_the_slack_are_pending() {
    let t = |keys_written, started| NicTimes {
        keys_written,
        started,
    };
    assert!(!pending(&t(1000, 1000)));
    assert!(!pending(&t(500, 1000)));
    assert!(!pending(&t(1000 + SLACK_SECS, 1000)));
    assert!(pending(&t(1000 + SLACK_SECS + 1, 1000)));
    // A start time near the top of the range does not wrap.
    assert!(!pending(&t(5, u64::MAX)));
}

#[test]
fn a_filetime_reads_as_unix_seconds() {
    assert_eq!(filetime_secs(0), 0);
    // 1970-01-01 exactly, and one second later.
    assert_eq!(filetime_secs(116_444_736_000_000_000), 0);
    assert_eq!(filetime_secs(116_444_736_010_000_000), 1);
    // 2026-11-25 00:00:00 UTC.
    assert_eq!(filetime_secs(134_400_384_000_000_000), 1_795_564_800);
}

#[test]
fn a_value_written_after_the_nic_started_says_so_even_though_it_reads_fine() {
    let p = profile(NIC);
    let pc = nic_pc("0", 2000, 1000);
    let lines = restart_lines(&p, &pc);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("written, NIC not restarted")
            && lines[0].contains("1000 s after the adapter started"),
        "{}",
        lines[0]
    );
    assert!(!lines[0].starts_with(MUST_HAVE), "{}", lines[0]);
    // The only line: the value itself is as the profile wants.
    assert_eq!(check(&p, &pc).len(), 1);
}

#[test]
fn a_restarted_nic_gives_no_line() {
    let p = profile(NIC);
    assert!(check(&p, &nic_pc("0", 1000, 2000)).is_empty());
    assert!(check(&p, &nic_pc("0", 1005, 1000)).is_empty());
}

#[test]
fn a_wrong_value_is_its_own_problem_not_a_restart_line() {
    let p = profile(NIC);
    let pc = nic_pc("1", 2000, 1000);
    assert!(restart_lines(&p, &pc).is_empty());
    assert_eq!(check(&p, &pc).len(), 1);
}

#[test]
fn an_entry_off_the_nic_s_keys_never_gives_the_line() {
    let p = profile(
        "name = \"b\"\nnic_guid = \"{G}\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"X\"\ndword = 1\n",
    );
    let mut pc = nic_pc("0", 2000, 1000);
    pc.reg
        .insert(("HKLM\\A".into(), "X".into()), Value::Dword(1));
    assert!(check(&p, &pc).is_empty());
}

#[test]
fn an_entry_for_another_driver_never_gives_the_line() {
    let p = profile(&format!("{NIC}nic_service = \"rt640x64\"\n"));
    let mut pc = nic_pc("0", 2000, 1000);
    pc.nic_service = Some("rtu56cx22x64".into());
    assert!(check(&p, &pc).is_empty());
    pc.nic_service = Some("rt640x64".into());
    assert_eq!(check(&p, &pc).len(), 1);
}

#[test]
fn a_nic_with_no_times_or_not_found_gives_no_restart_line() {
    let p = profile(NIC);
    let mut pc = nic_pc("0", 2000, 1000);
    pc.nic_times = None;
    assert!(restart_lines(&p, &pc).is_empty());
    let mut gone = nic_pc("0", 2000, 1000);
    gone.nic_gone = true;
    assert!(restart_lines(&p, &gone).is_empty());
}

#[test]
fn times_that_cannot_be_read_are_a_line_not_a_pass() {
    let p = profile(NIC);
    let mut pc = nic_pc("0", 2000, 1000);
    pc.nic_times_err = Some("os error 5".into());
    let lines = restart_lines(&p, &pc);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("unreadable (os error 5)"), "{}", lines[0]);
    assert!(restart_check(&profile("name = \"b\"\n"), &pc).is_none());
}
