//! The process keeper (dasio#185): what it sets, what it logs, and the
//! `no_ini` key that drops a replaced tool's ini entries.

use super::*;
use crate::pc_profile::keep::{QUIET_MS, Quiet, Set, counted, held, line, utc_stamp, wanted};

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
fn a_process_set_back_again_logs_once_a_minute_with_the_count() {
    let mut q = Quiet::default();
    assert_eq!(q.note(5, 1_000), Some(1));
    assert_eq!(q.note(5, 3_000), None);
    assert_eq!(q.note(5, 1_000 + QUIET_MS - 1), None);
    // A minute after the logged one: this change and the two quieted.
    assert_eq!(q.note(5, 1_000 + QUIET_MS), Some(3));
    assert_eq!(q.note(5, 1_000 + QUIET_MS + 10), None);
    // Another process is its own.
    assert_eq!(q.note(6, 3_000), Some(1));
    // A process gone is forgotten: its id logs at once when it comes back.
    q.keep_only(&[6]);
    assert_eq!(q.note(5, 1_000 + QUIET_MS + 20), Some(1));
    assert_eq!(q.note(6, 4_000), None);
}

#[test]
fn a_counted_line_says_something_sets_it_back() {
    assert_eq!(counted("kept a".into(), 1), "kept a");
    assert_eq!(
        counted("kept a".into(), 3),
        "kept a (3 changes since the last line: something else sets it back)"
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
