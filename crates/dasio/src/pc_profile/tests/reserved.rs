//! A written `ReservedCpuSets` is not a reserved CPU until Windows boots
//! (dasio#237).

use super::super::reserved::{KEY, NAME, mask};
use super::*;

/// A site profile with one `ReservedCpuSets` entry on `key`, named `name`
/// (CPUs 2-3: alex-nb's value).
fn profile(key: &str, name: &str) -> Profile {
    parse_with(
        &format!(
            "name = \"nb\"\nnic_guid = \"{{G}}\"\n[[registry]]\nkey = '{key}'\nname = \"{name}\"\nbinary = \"0c00000000000000\"\n"
        ),
        false,
    )
    .unwrap()
}

/// A PC whose `key`\`name` reads `value` and whose Windows reserves
/// `reserved` (`None`: nothing to read).
fn pc(key: &str, name: &str, value: &[u8], reserved: Option<u64>) -> Pc {
    let mut pc = Pc::default();
    pc.reg
        .insert((key.into(), name.into()), Value::Binary(value.to_vec()));
    pc.reserved = reserved;
    pc
}

const CPUS_2_3: [u8; 8] = [0x0c, 0, 0, 0, 0, 0, 0, 0];

fn reserved_lines(p: &Profile, pc: &Pc) -> Vec<String> {
    check(p, pc)
        .iter()
        .filter(|x| x.what.starts_with("dasio's reserved CPUs"))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_value_s_mask_is_its_first_eight_bytes_little_endian() {
    assert_eq!(mask(&CPUS_2_3), 0x0c);
    // FOH's own value, CPUs 4-31, written as four bytes.
    assert_eq!(mask(&[0xf0, 0xff, 0xff, 0xff]), 0xffff_fff0);
    assert_eq!(mask(&[]), 0);
    assert_eq!(mask(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), 0x0807_0605_0403_0201);
}

#[test]
fn a_written_value_windows_has_not_read_yet_is_a_pending_reboot() {
    let p = profile(KEY, NAME);
    // alex-nb after `apply`, before its reboot: nothing reserved.
    let lines = reserved_lines(&p, &pc(KEY, NAME, &CPUS_2_3, Some(0)));
    assert_eq!(
        lines,
        vec![
            "dasio's reserved CPUs (ReservedCpuSets): written (0xc) but Windows reserves 0x0 now: it reads the value at boot, so a reboot is pending (the profile wants CPUs 0xc reserved: a reboot after the write (dasio#237)) — Windows reads ReservedCpuSets at boot only; until then other processes' threads run on dasio's CPUs".to_string()
        ]
    );
    // One of the two CPUs reserved is still pending.
    let half = reserved_lines(&p, &pc(KEY, NAME, &CPUS_2_3, Some(0x4)));
    assert_eq!(half.len(), 1, "{half:?}");
    assert!(
        half[0].contains("written (0xc) but Windows reserves 0x4 now"),
        "{half:?}"
    );
    // A WARN, not a must-have.
    assert!(
        check(&p, &pc(KEY, NAME, &CPUS_2_3, Some(0)))
            .iter()
            .filter(|x| x.what.starts_with("dasio's reserved CPUs"))
            .all(|x| !x.must)
    );
}

#[test]
fn reserved_cpus_after_the_boot_are_no_problem() {
    let p = profile(KEY, NAME);
    assert_eq!(
        reserved_lines(&p, &pc(KEY, NAME, &CPUS_2_3, Some(0x0c))),
        Vec::<String>::new()
    );
    // More reserved than the value names (another instance's CPUs too).
    assert_eq!(
        reserved_lines(&p, &pc(KEY, NAME, &CPUS_2_3, Some(0xff))),
        Vec::<String>::new()
    );
    // Nothing to read (another OS): nothing said.
    assert_eq!(
        reserved_lines(&p, &pc(KEY, NAME, &CPUS_2_3, None)),
        Vec::<String>::new()
    );
}

#[test]
fn only_the_reserved_cpu_sets_value_read_as_wanted_is_judged() {
    // A value that reads otherwise is its own problem, not a pending reboot.
    let other = [0x0f, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        reserved_lines(&profile(KEY, NAME), &pc(KEY, NAME, &other, Some(0))),
        Vec::<String>::new()
    );
    // An absent value too.
    let none = Pc {
        reserved: Some(0),
        ..Pc::default()
    };
    assert_eq!(
        reserved_lines(&profile(KEY, NAME), &none),
        Vec::<String>::new()
    );
    // Another value on the same key, the same name on another key.
    assert_eq!(
        reserved_lines(
            &profile(KEY, "Other"),
            &pc(KEY, "Other", &CPUS_2_3, Some(0))
        ),
        Vec::<String>::new()
    );
    let elsewhere = r"HKLM\SYSTEM\CurrentControlSet\Control\Elsewhere";
    assert_eq!(
        reserved_lines(
            &profile(elsewhere, NAME),
            &pc(elsewhere, NAME, &CPUS_2_3, Some(0))
        ),
        Vec::<String>::new()
    );
    // A profile without the entry says nothing.
    let bare = parse_with("name = \"nb\"\nnic_guid = \"{G}\"\n", false).unwrap();
    assert_eq!(
        reserved_lines(&bare, &pc(KEY, NAME, &CPUS_2_3, Some(0))),
        Vec::<String>::new()
    );
}

#[test]
fn an_unreadable_reservation_says_so() {
    let p = profile(KEY, NAME);
    let mut gone = pc(KEY, NAME, &CPUS_2_3, None);
    gone.reserved_err = Some("no CPU set information".into());
    let lines = reserved_lines(&p, &gone);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with(
            "dasio's reserved CPUs (ReservedCpuSets): unreadable (no CPU set information): cannot tell whether the written ReservedCpuSets is in effect"
        ),
        "{lines:?}"
    );
}
