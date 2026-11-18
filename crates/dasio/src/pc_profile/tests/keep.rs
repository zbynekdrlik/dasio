//! The process keeper (dasio#185): what it sets, what it logs, and the
//! `no_ini` key that drops a replaced tool's ini entries.

use super::*;
use crate::pc_profile::keep::{
    FIGHT_LIMIT, FIGHT_WINDOW_MS, Fights, Note, Set, gave_up_line, held, line, mask_outside,
    utc_stamp, wanted,
};

fn entry(exe: &str, affinity: Option<&str>, priority: Option<&str>) -> ProcEntry {
    ProcEntry {
        exe: exe.into(),
        affinity: affinity.map(String::from),
        priority: priority.map(String::from),
        integrity: None,
        why: String::new(),
    }
}

fn state(affinity: u64, priority: &str) -> ProcState {
    ProcState {
        affinity,
        priority: priority.into(),
        integrity: Ok("medium".into()),
    }
}

#[test]
fn the_keeper_holds_entries_with_an_affinity_or_a_priority_only() {
    let p = parse(
        "name = \"dsg\"\n\
         [[process]]\nexe = \"a.exe\"\naffinity = \"0x1\"\n\
         [[process]]\nexe = \"b.exe\"\npriority = \"high\"\n\
         [[process]]\nexe = \"c.exe\"\nintegrity = \"medium\"\n",
    )
    .unwrap();
    let names: Vec<&str> = held(&p).iter().map(|e| e.exe.as_str()).collect();
    assert_eq!(names, ["a.exe", "b.exe"]);
}

#[test]
fn only_what_differs_is_set() {
    let e = entry("SGDawNodeService.exe", Some("0x1"), Some("high"));
    assert_eq!(wanted(&e, &state(0x1, "high")), None);
    assert_eq!(
        wanted(&e, &state(0x1, "below normal")),
        Some(Set {
            affinity: None,
            priority: Some("high".into())
        })
    );
    assert_eq!(
        wanted(&e, &state(0xF, "high")),
        Some(Set {
            affinity: Some(0x1),
            priority: None
        })
    );
    assert_eq!(
        wanted(&e, &state(0xF, "normal")),
        Some(Set {
            affinity: Some(0x1),
            priority: Some("high".into())
        })
    );
    // An entry naming only one of them never sets the other.
    assert_eq!(
        wanted(&entry("x.exe", None, Some("high")), &state(0xF, "normal")),
        Some(Set {
            affinity: None,
            priority: Some("high".into())
        })
    );
    assert_eq!(
        wanted(&entry("x.exe", Some("0x2"), None), &state(0x2, "idle")),
        None
    );
}

#[test]
fn a_change_logs_what_moved_from_what() {
    let s = state(0x1, "below normal");
    let both = Set {
        affinity: Some(0x8),
        priority: Some("high".into()),
    };
    assert_eq!(
        line("SGDawNodeService.exe", 3792, &s, &both),
        "kept SGDawNodeService.exe (pid 3792): priority below normal -> high, affinity 0x1 -> 0x8"
    );
    let only = Set {
        affinity: Some(0xE),
        priority: None,
    };
    assert_eq!(
        line("dasio-dsg.exe", 7, &s, &only),
        "kept dasio-dsg.exe (pid 7): affinity 0x1 -> 0xE"
    );
}

#[test]
fn a_process_set_back_again_and_again_is_given_up() {
    let mut f = Fights::default();
    for n in 1..FIGHT_LIMIT {
        assert_eq!(f.note(5, u64::from(n) * 2_000), Note::Changed, "change {n}");
    }
    assert!(!f.gave_up(5));
    assert_eq!(f.note(5, 20_000), Note::GiveUp);
    assert!(f.gave_up(5));
    // Another process is its own.
    assert!(!f.gave_up(6));
    assert_eq!(f.note(6, 20_000), Note::Changed);
    // A process gone is forgotten: its id is held again when it comes back.
    f.keep_only(&[6]);
    assert!(!f.gave_up(5));
    assert_eq!(f.note(5, 22_000), Note::Changed);
}

#[test]
fn the_count_starts_over_with_each_window() {
    // Four changes a window, for two windows: never the fifth in one.
    let mut f = Fights::default();
    for n in 0..(2 * FIGHT_LIMIT) {
        assert_eq!(
            f.note(
                9,
                u64::from(n) * (FIGHT_WINDOW_MS / u64::from(FIGHT_LIMIT - 1))
            ),
            Note::Changed,
            "change {n}"
        );
    }
    assert!(!f.gave_up(9));
}

#[test]
fn a_given_up_process_names_why() {
    assert_eq!(
        gave_up_line("AbletonLiveMaster.exe", 12),
        "gave up keeping AbletonLiveMaster.exe (pid 12): something else set it back 5 times in 10 min; the PC profile check shows what differs"
    );
}

#[test]
fn a_mask_naming_a_cpu_the_pc_lacks_is_refused() {
    assert_eq!(mask_outside(0x1, 0xF), None);
    assert_eq!(mask_outside(0xF, 0xF), None);
    assert_eq!(
        mask_outside(0xFFFF_0000, 0xFFFF),
        Some("affinity 0xFFFF0000 names CPUs this PC does not have (it has 0xFFFF)".into())
    );
    assert_eq!(
        mask_outside(0x11, 0x1),
        Some("affinity 0x11 names CPUs this PC does not have (it has 0x1)".into())
    );
}

#[test]
fn log_stamps_are_utc_to_the_second() {
    assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
    // 18.11.2026 17:19:31Z.
    assert_eq!(utc_stamp(1_795_022_371), "2026-11-18T17:19:31Z");
}

#[test]
fn no_ini_drops_the_baseline_s_and_the_site_s_entries_for_that_file() {
    let lasso = r"C:\ProgramData\ProcessLasso\config\prolasso.ini";
    let base = parse_with(
        &format!(
            "name = \"base\"\n[[ini]]\nfile = '{lasso}'\nkey = \"A\"\nvalue = \"1\"\n\
             [[ini]]\nfile = 'C:\\other.ini'\nkey = \"B\"\nvalue = \"2\"\n"
        ),
        false,
    )
    .unwrap();
    let site = parse(&format!(
        "name = \"dsg\"\nno_ini = ['{}']\n[[ini]]\nfile = '{lasso}'\nkey = \"C\"\nvalue = \"3\"\n",
        lasso.to_ascii_uppercase()
    ))
    .unwrap();
    let merged = merge(&base, &site);
    let keys: Vec<&str> = merged.ini.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["B"]);
    // Without no_ini both files' entries stay.
    let plain = parse(&format!(
        "name = \"dsg\"\n[[ini]]\nfile = '{lasso}'\nkey = \"C\"\nvalue = \"3\"\n"
    ))
    .unwrap();
    let keys: Vec<String> = merge(&base, &plain)
        .ini
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(keys, ["A", "B", "C"]);
}

#[test]
fn install_and_uninstall_stop_the_keeper_through_its_own_stop_file() {
    // The scripts name the file the keeper looks for: under ProgramData,
    // in dasio's folder.
    let tail = crate::pc_profile::keep::STOP_PATH
        .strip_prefix(r"C:\ProgramData\")
        .expect("the stop file is under ProgramData");
    for script in ["install.ps1", "uninstall.ps1"] {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../scripts/{script}")),
        )
        .unwrap();
        assert!(
            text.contains(&format!("Join-Path $env:ProgramData \"{tail}\"")),
            "{script} does not name {tail}"
        );
    }
}
