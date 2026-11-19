//! The keeper's catch-all, `others_affinity` (dasio#185: Process Lasso's
//! `*,0,0-1` on mbc): the profile key, which processes it takes, what it
//! sets and logs, and the check's one summary problem.

use super::*;
use crate::pc_profile::keep::{
    GRACE_MS, NEVER, Processes, SHOWN, Seen, age_ms, catch_all_takes, dasio_exe, others,
    others_line, others_loaded, others_problem, others_wanted,
};

/// mbc: 12 CPUs, 1 instance (system 0-1, dasio 2-3, Live 4-11).
const MBC: &str = "name = \"mbc\"\nothers_affinity = \"{system}\"\n\
[[process]]\nexe = \"Ableton Live 12 Suite.exe\"\naffinity = \"{daw}\"\n";

fn mbc() -> Profile {
    let mut p = parse(MBC).unwrap();
    resolve(&mut p, 12).unwrap();
    p
}

/// Process ids no test process has (above any OS's pid range in use), so
/// `check`'s own-process rule never meets one.
const PID: u32 = 4_000_000_000;

/// Old enough to count.
const OLD: u64 = GRACE_MS;

fn seen(pid: u32, exe: &str, affinity: u64, age_ms: u64) -> Seen {
    Seen {
        pid,
        exe: exe.into(),
        affinity,
        age_ms,
    }
}

/// mbc's processes as the check reads them.
fn fake(list: Vec<Seen>) -> Pc {
    Pc {
        running: Processes {
            system: 0xFFF,
            list,
        },
        ..Pc::default()
    }
}

#[test]
fn the_key_takes_a_mask_or_a_layout_placeholder_resolved_on_the_pc() {
    assert_eq!(mbc().others_affinity.as_deref(), Some("0x3"));
    let resolved = |v: &str| {
        let mut p = parse(&format!("name = \"x\"\nothers_affinity = \"{v}\"\n")).unwrap();
        resolve(&mut p, 12).unwrap();
        p.others_affinity
    };
    assert_eq!(resolved("{daw}").as_deref(), Some("0xFF0"));
    assert_eq!(resolved("{dasio:1}").as_deref(), Some("0xC"));
    assert_eq!(resolved("0x3").as_deref(), Some("0x3"));
    let mut none = parse("name = \"x\"\n").unwrap();
    resolve(&mut none, 12).unwrap();
    assert_eq!(none.others_affinity, None);
}

#[test]
fn a_bad_value_refuses_the_profile_with_its_whole_message() {
    let bad = |v: &str| parse(&format!("name = \"x\"\nothers_affinity = \"{v}\"\n")).unwrap_err();
    assert_eq!(
        bad("zz"),
        "others_affinity zz is not a hex mask, {system}, {dasio:N} or {daw}"
    );
    assert_eq!(
        bad("{dasio:x}"),
        "others_affinity {dasio:x} is not a hex mask, {system}, {dasio:N} or {daw}"
    );
    assert_eq!(bad("0x0"), "others_affinity 0x0 names no CPU");
    assert_eq!(bad("0"), "others_affinity 0 names no CPU");
    // A placeholder the layout cannot fill refuses the load.
    let mut p = parse("name = \"x\"\nothers_affinity = \"{dasio:2}\"\n").unwrap();
    assert_eq!(
        resolve(&mut p, 12).unwrap_err(),
        "others_affinity: dasio:2: the profile has instances = 1 (1 to 1)"
    );
}

#[test]
fn the_site_s_catch_all_wins_over_the_baseline_s() {
    let base = parse_with("name = \"b\"\nothers_affinity = \"0x1\"\n", false).unwrap();
    let site = parse("name = \"pc\"\nothers_affinity = \"0x3\"\n").unwrap();
    assert_eq!(merge(&base, &site).others_affinity.as_deref(), Some("0x3"));
    let plain = parse("name = \"pc\"\n").unwrap();
    assert_eq!(merge(&base, &plain).others_affinity.as_deref(), Some("0x1"));
    // The built-in baseline names none: no PC gets one it did not ask for.
    assert_eq!(baseline().unwrap().others_affinity, None);
    assert_eq!(baseline_only(&baseline().unwrap()).others_affinity, None);
}

#[test]
fn a_media_process_s_own_check_never_reads_every_process() {
    // A bridge checks in its own process: the catch-all's read is left to
    // `dasio pc-profile check` and the keeper.
    let p = mbc();
    assert_eq!(in_process(&p).others_affinity, None);
    let pc = fake(vec![seen(PID, "notepad.exe", 0xFFF, OLD)]);
    assert!(check(&in_process(&p), &pc).is_empty());
    assert_eq!(pc.processes_reads.get(), 0);
}

#[test]
fn the_key_counts_as_one_entry() {
    let with = parse("name = \"x\"\nothers_affinity = \"0x3\"\n").unwrap();
    assert_eq!(with.entries(), 1);
    let disk = parse("name = \"x\"\n[[disk]]\ndrive = \"C:\"\nmin_free_gb = 1\n").unwrap();
    assert_eq!(disk.entries(), 1);
}

#[test]
fn the_catch_all_takes_every_other_process() {
    let p = mbc();
    for exe in [
        "chrome.exe",
        "AUDIODG.EXE",
        "svchost.exe",
        "dwm.exe",
        "dsync.exe",
    ] {
        assert!(catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
}

#[test]
fn a_process_an_entry_names_is_never_the_catch_all_s() {
    let p = mbc();
    assert!(!catch_all_takes(&p, "ABLETON LIVE 12 SUITE.EXE", 100, 1));
    // A check-only entry names it too: the baseline's integrity of both
    // Lives keeps a DAW off the system CPUs.
    let p = parse(
        "name = \"x\"\nothers_affinity = \"0x3\"\n\
         [[process]]\nexe = \"AbletonLiveMaster.exe\"\nintegrity = \"medium\"\n",
    )
    .unwrap();
    assert!(!catch_all_takes(&p, "abletonlivemaster.exe", 100, 1));
    assert!(catch_all_takes(&p, "Ableton Index.exe", 100, 1));
}

#[test]
fn dasio_s_own_processes_and_the_caller_are_never_taken() {
    let p = mbc();
    for exe in [
        "dasio.exe",
        "DASIO-KEEP.EXE",
        "dasio-mbc-snv.exe",
        "dasio-band-asio-dante.exe",
    ] {
        assert!(dasio_exe(exe), "{exe}");
        assert!(!catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
    for exe in ["dasiox.exe", "my-dasio.exe", "dtray.exe"] {
        assert!(!dasio_exe(exe), "{exe}");
        assert!(catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
    // The caller (the keeper itself, the checking process), by its id.
    assert!(!catch_all_takes(&p, "notepad.exe", 7, 7));
    assert!(catch_all_takes(&p, "notepad.exe", 7, 8));
}

#[test]
fn the_kernel_s_pseudo_processes_are_never_taken() {
    let p = mbc();
    for exe in [
        "[System Process]",
        "System",
        "Registry",
        "Memory Compression",
        "Secure System",
        "vmmem",
    ] {
        assert!(!catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
}

#[test]
fn the_session_0_core_is_never_taken_its_children_are() {
    let p = mbc();
    for exe in [
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "services.exe",
        "lsass.exe",
        "WINLOGON.EXE",
    ] {
        assert!(!catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
    assert_eq!(NEVER.len(), 6);
    for exe in ["svchost.exe", "userinit.exe", "LogonUI.exe", "explorer.exe"] {
        assert!(catch_all_takes(&p, exe, 100, 1), "{exe}");
    }
}

#[test]
fn only_a_process_on_every_cpu_is_set_and_only_when_it_differs() {
    // mbc: 12 CPUs, the catch-all 0-1.
    assert_eq!(others_wanted(0x3, 0xFFF, 0xFFF), Some(0x3));
    // Set already (by the keeper): nothing.
    assert_eq!(others_wanted(0x3, 0x3, 0xFFF), None);
    // Its own mask (a child of Live on 4-11, a tool pinned by hand).
    assert_eq!(others_wanted(0x3, 0xFF0, 0xFFF), None);
    assert_eq!(others_wanted(0x3, 0x1, 0xFFF), None);
    // A catch-all of every CPU sets nothing.
    assert_eq!(others_wanted(0xFFF, 0xFFF, 0xFFF), None);
}

#[test]
fn a_mask_naming_cpus_the_pc_lacks_is_no_catch_all() {
    let p = mbc();
    assert_eq!(others(&p, 0xFFF), Ok(Some(0x3)));
    assert_eq!(
        others(&p, 0x1),
        Err("others_affinity 0x3 names CPUs this PC does not have (it has 0x1)".into())
    );
    assert_eq!(others(&parse("name = \"x\"\n").unwrap(), 0xFFF), Ok(None));
    // Parsed but not resolved: a placeholder is no mask yet.
    let raw = parse("name = \"x\"\nothers_affinity = \"{system}\"\n").unwrap();
    assert_eq!(
        others(&raw, 0xFFF),
        Err("others_affinity {system} is not a mask (not resolved)".into())
    );
}

#[test]
fn the_keeper_says_its_catch_all_once_at_a_load_and_each_process_set() {
    let p = mbc();
    assert_eq!(
        others_loaded(&p, Some(0xFFF)).as_deref(),
        Some(
            "every process no [[process]] entry names goes on 0x3 (others_affinity), unless it set its own affinity"
        )
    );
    assert_eq!(
        others_loaded(&p, Some(0x1)).as_deref(),
        Some(
            "the catch-all holds nothing: others_affinity 0x3 names CPUs this PC does not have (it has 0x1)"
        )
    );
    assert_eq!(
        others_loaded(&p, None).as_deref(),
        Some(
            "the catch-all holds nothing: the PC's CPUs are unreadable or in more than one processor group (a 64-bit mask cannot name them)"
        )
    );
    let plain = parse("name = \"x\"\n").unwrap();
    assert_eq!(others_loaded(&plain, Some(0xFFF)), None);
    assert_eq!(others_loaded(&plain, None), None);
    assert_eq!(
        others_line("notepad.exe", 12, 0xFFF, 0x3),
        "kept notepad.exe (pid 12): affinity 0xFFF -> 0x3 (others_affinity)"
    );
}

#[test]
fn a_process_s_age_comes_from_its_start_in_filetime_units() {
    assert_eq!(age_ms(10_000, 30_000), 2);
    assert_eq!(age_ms(0, 100_000_000), 10_000);
    // A start after the read (a clock step): age 0.
    assert_eq!(age_ms(30_000, 10_000), 0);
    // The grace the check gives a new process: 5 keeper passes, 10 s (the
    // rule's text).
    assert_eq!(GRACE_MS, 10_000);
}

#[test]
fn every_process_held_is_no_problem() {
    let p = mbc();
    let pc = fake(vec![
        seen(PID, "chrome.exe", 0x3, OLD),
        seen(PID + 1, "Ableton Live 12 Suite.exe", 0xFF0, OLD),
        seen(PID + 2, "tool.exe", 0xF0, OLD),
    ]);
    assert_eq!(others_problem(&p, &pc, 1), None);
    assert!(check(&p, &pc).is_empty());
}

#[test]
fn processes_still_on_every_cpu_are_one_problem_naming_their_exes() {
    let p = mbc();
    let pc = fake(vec![
        seen(PID, "notepad.exe", 0xFFF, OLD),
        seen(PID + 1, "Chrome.exe", 0xFFF, OLD),
        seen(PID + 2, "chrome.exe", 0xFFF, OLD + 1),
        seen(PID + 3, "held.exe", 0x3, OLD),
        // Not the catch-all's: an entry's, dasio's, the session-0 core, a
        // pseudo-process, a process on its own mask.
        seen(PID + 4, "Ableton Live 12 Suite.exe", 0xFFF, OLD),
        seen(PID + 5, "dasio-mbc-snv.exe", 0xFFF, OLD),
        seen(PID + 6, "lsass.exe", 0xFFF, OLD),
        seen(PID + 7, "System", 0xFFF, OLD),
        seen(PID + 8, "tool.exe", 0xF0, OLD),
        // Too young: the keeper has not had its passes on it.
        seen(PID + 9, "new.exe", 0xFFF, GRACE_MS - 1),
    ]);
    let texts: Vec<String> = check(&p, &pc).iter().map(ToString::to_string).collect();
    assert_eq!(
        texts,
        [
            "others_affinity: not held: Chrome.exe, notepad.exe on every CPU (0xFFF) (the profile wants 0x3) — every process no [[process]] entry names, held by dasio's keeper as Process Lasso's * rule did (dasio#185)"
        ]
    );
    // The checking process itself is never counted.
    assert_eq!(
        others_problem(&p, &pc, PID).unwrap().found,
        "not held: Chrome.exe on every CPU (0xFFF)"
    );
}

#[test]
fn past_five_exes_the_problem_says_how_many_more() {
    let p = mbc();
    let names = [
        "a.exe", "b.exe", "c.exe", "d.exe", "e.exe", "f.exe", "g.exe",
    ];
    let list = |n: usize| {
        fake(
            names[..n]
                .iter()
                .zip(PID..)
                .map(|(exe, pid)| seen(pid, exe, 0xFFF, OLD))
                .collect(),
        )
    };
    assert_eq!(
        others_problem(&p, &list(7), 1).unwrap().found,
        "not held: a.exe, b.exe, c.exe, d.exe, e.exe and 2 more on every CPU (0xFFF)"
    );
    assert_eq!(
        others_problem(&p, &list(SHOWN), 1).unwrap().found,
        "not held: a.exe, b.exe, c.exe, d.exe, e.exe on every CPU (0xFFF)"
    );
}

#[test]
fn without_the_key_no_process_is_read() {
    let pc = fake(vec![seen(PID, "notepad.exe", 0xFFF, OLD)]);
    assert!(check(&parse("name = \"x\"\n").unwrap(), &pc).is_empty());
    assert_eq!(pc.processes_reads.get(), 0);
    assert_eq!(check(&mbc(), &pc).len(), 1);
    assert_eq!(pc.processes_reads.get(), 1);
}

#[test]
fn an_unreadable_list_or_a_mask_the_pc_lacks_is_one_problem() {
    let p = mbc();
    let pc = Pc {
        processes_err: Some("the process list is unreadable (x)".into()),
        ..Pc::default()
    };
    let pr = others_problem(&p, &pc, 1).unwrap();
    assert_eq!(
        (pr.found.as_str(), pr.want.as_str()),
        ("unreadable (the process list is unreadable (x))", "0x3")
    );
    let small = Pc {
        running: Processes {
            system: 0x1,
            list: vec![seen(PID, "notepad.exe", 0x1, OLD)],
        },
        ..Pc::default()
    };
    let pr = others_problem(&p, &small, 1).unwrap();
    assert_eq!(
        (pr.found.as_str(), pr.want.as_str()),
        (
            "others_affinity 0x3 names CPUs this PC does not have (it has 0x1)",
            "CPUs this PC has"
        )
    );
}
