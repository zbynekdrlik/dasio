//! The keeper's I/O, memory and GPU priority and thread priority boost
//! (dasio#185: what Process Lasso set on FOH beyond affinity and priority
//! class): the profile's names, the check, what the keeper sets and logs.

use super::*;
use crate::pc_profile::keep::{Set, held, line, wanted};

const CG: &str = "name = \"foh\"\n[[process]]\nexe = \"cg.exe\"\nio_priority = \"very low\"\nmemory_priority = \"very low\"\n\
[[process]]\nexe = \"chrome.exe\"\ngpu_priority = \"below normal\"\npriority_boost = false\n";

fn state(extra: Extras) -> ProcState {
    ProcState {
        affinity: 0xF,
        priority: "normal".into(),
        integrity: Ok("medium".into()),
        extra,
    }
}

fn extras(
    io: Option<&str>,
    memory: Option<&str>,
    gpu: Option<&str>,
    boost: Option<bool>,
) -> Extras {
    Extras {
        io: io.map(String::from),
        memory: memory.map(String::from),
        gpu: gpu.map(String::from),
        boost,
    }
}

#[test]
fn levels_read_by_value_and_by_name() {
    assert_eq!(level_name(&IO_PRIORITIES, 0, 0), Some("very low"));
    assert_eq!(level_name(&IO_PRIORITIES, 0, 3), Some("high"));
    // 4 is critical, the memory manager's own: never a name.
    assert_eq!(level_name(&IO_PRIORITIES, 0, 4), None);
    assert_eq!(level_name(&MEMORY_PRIORITIES, 1, 1), Some("very low"));
    assert_eq!(level_name(&MEMORY_PRIORITIES, 1, 5), Some("normal"));
    assert_eq!(level_name(&MEMORY_PRIORITIES, 1, 0), None);
    assert_eq!(level_name(&GPU_PRIORITIES, 0, 4), Some("high"));
    assert_eq!(level_value(&IO_PRIORITIES, 0, "high"), Some(3));
    assert_eq!(level_value(&MEMORY_PRIORITIES, 1, "very low"), Some(1));
    assert_eq!(level_value(&MEMORY_PRIORITIES, 1, "below normal"), Some(4));
    assert_eq!(level_value(&GPU_PRIORITIES, 0, "below normal"), Some(1));
    assert_eq!(level_value(&GPU_PRIORITIES, 0, "turbo"), None);
}

#[test]
fn a_profile_names_them_and_refuses_a_name_not_on_the_list() {
    let p = parse(CG).unwrap();
    assert_eq!(p.process[0].io_priority.as_deref(), Some("very low"));
    assert_eq!(p.process[0].memory_priority.as_deref(), Some("very low"));
    assert_eq!(p.process[1].gpu_priority.as_deref(), Some("below normal"));
    assert_eq!(p.process[1].priority_boost, Some(false));
    let bad = |key: &str, v: &str| {
        parse(&format!(
            "name = \"x\"\n[[process]]\nexe = \"a.exe\"\n{key} = \"{v}\"\n"
        ))
        .unwrap_err()
    };
    assert_eq!(
        bad("io_priority", "critical"),
        r#"process a.exe: io_priority critical is not one of ["very low", "low", "normal", "high"]"#
    );
    assert_eq!(
        bad("memory_priority", "high"),
        r#"process a.exe: memory_priority high is not one of ["very low", "low", "medium", "below normal", "normal"]"#
    );
    assert_eq!(
        bad("gpu_priority", "max"),
        r#"process a.exe: gpu_priority max is not one of ["idle", "below normal", "normal", "above normal", "high", "realtime"]"#
    );
}

#[test]
fn the_keeper_holds_an_entry_naming_only_one_of_them() {
    let p = parse(
        "name = \"x\"\n\
         [[process]]\nexe = \"a.exe\"\nio_priority = \"high\"\n\
         [[process]]\nexe = \"b.exe\"\nmemory_priority = \"low\"\n\
         [[process]]\nexe = \"c.exe\"\ngpu_priority = \"high\"\n\
         [[process]]\nexe = \"d.exe\"\npriority_boost = false\n\
         [[process]]\nexe = \"e.exe\"\nintegrity = \"medium\"\n",
    )
    .unwrap();
    let names: Vec<&str> = held(&p).iter().map(|e| e.exe.as_str()).collect();
    assert_eq!(names, ["a.exe", "b.exe", "c.exe", "d.exe"]);
    assert!(p.process.iter().all(ProcEntry::checks));
}

#[test]
fn only_what_was_read_and_differs_is_set() {
    let p = parse(CG).unwrap();
    let (cg, chrome) = (&p.process[0], &p.process[1]);
    let held = state(extras(Some("very low"), Some("very low"), None, None));
    assert_eq!(wanted(cg, &held), None);
    assert_eq!(
        wanted(
            cg,
            &state(extras(Some("normal"), Some("normal"), None, None))
        ),
        Some(Set {
            io: Some("very low".into()),
            memory: Some("very low".into()),
            ..Set::default()
        })
    );
    // One read as unreadable is left alone: the check names it.
    assert_eq!(
        wanted(cg, &state(extras(None, Some("normal"), None, None))),
        Some(Set {
            memory: Some("very low".into()),
            ..Set::default()
        })
    );
    assert_eq!(
        wanted(
            chrome,
            &state(extras(None, None, Some("normal"), Some(true)))
        ),
        Some(Set {
            gpu: Some("below normal".into()),
            boost: Some(false),
            ..Set::default()
        })
    );
    assert_eq!(
        wanted(
            chrome,
            &state(extras(None, None, Some("below normal"), Some(false)))
        ),
        None
    );
    assert_eq!(wanted(chrome, &state(Extras::default())), None);
}

#[test]
fn a_change_logs_each_of_them_from_what() {
    let s = state(extras(Some("normal"), None, Some("normal"), Some(true)));
    let set = Set {
        io: Some("very low".into()),
        memory: Some("very low".into()),
        gpu: Some("below normal".into()),
        boost: Some(false),
        ..Set::default()
    };
    assert_eq!(
        line("cg.exe", 9, &s, &set),
        "kept cg.exe (pid 9): io priority normal -> very low, memory priority ? -> very low, \
         gpu priority normal -> below normal, priority boost on -> off"
    );
    assert!(Set::default().is_empty());
    assert!(!set.is_empty());
}

#[test]
fn the_check_names_each_that_differs_or_cannot_be_read() {
    let p = parse(CG).unwrap();
    let mut pc = Pc::default();
    pc.procs.insert(
        "cg.exe".into(),
        state(extras(Some("normal"), None, None, None)),
    );
    pc.procs.insert(
        "chrome.exe".into(),
        state(extras(None, None, Some("below normal"), Some(true))),
    );
    let found: Vec<(String, String, String)> = check(&p, &pc)
        .into_iter()
        .map(|x| (x.what, x.found, x.want))
        .collect();
    let row = |w: &str, f: &str, t: &str| (w.to_string(), f.to_string(), t.to_string());
    assert_eq!(
        found,
        vec![
            row("process cg.exe io priority", "normal", "very low"),
            row("process cg.exe memory priority", "unreadable", "very low"),
            row("process chrome.exe priority boost", "on", "off"),
        ]
    );
    // Held: nothing to report.
    pc.procs.insert(
        "cg.exe".into(),
        state(extras(Some("very low"), Some("very low"), None, None)),
    );
    pc.procs.insert(
        "chrome.exe".into(),
        state(extras(None, None, Some("below normal"), Some(false))),
    );
    assert_eq!(check(&p, &pc), Vec::new());
}

#[test]
fn a_site_entry_takes_over_only_the_fields_it_sets() {
    let base = parse_with(
        "name = \"base\"\n[[process]]\nexe = \"cg.exe\"\nio_priority = \"very low\"\nmemory_priority = \"low\"\ngpu_priority = \"idle\"\npriority_boost = true\n",
        false,
    )
    .unwrap();
    let site = parse(
        "name = \"foh\"\n[[process]]\nexe = \"CG.exe\"\nmemory_priority = \"very low\"\npriority_boost = false\n",
    )
    .unwrap();
    let m = merge(&base, &site);
    let cg: Vec<&ProcEntry> = m
        .process
        .iter()
        .filter(|e| e.exe.eq_ignore_ascii_case("cg.exe"))
        .collect();
    assert_eq!(cg.len(), 2);
    // The baseline's keeps what the site leaves to it.
    assert_eq!(cg[0].io_priority.as_deref(), Some("very low"));
    assert_eq!(cg[0].gpu_priority.as_deref(), Some("idle"));
    assert_eq!(cg[0].memory_priority, None);
    assert_eq!(cg[0].priority_boost, None);
    assert_eq!(cg[1].memory_priority.as_deref(), Some("very low"));
    assert_eq!(cg[1].priority_boost, Some(false));
    // A baseline entry the site took over completely goes.
    let all = parse(
        "name = \"foh\"\n[[process]]\nexe = \"cg.exe\"\nio_priority = \"low\"\nmemory_priority = \"low\"\ngpu_priority = \"low\"\npriority_boost = true\n",
    );
    assert!(all.is_err(), "\"low\" is no GPU priority");
    let all = parse(
        "name = \"foh\"\n[[process]]\nexe = \"cg.exe\"\nio_priority = \"low\"\nmemory_priority = \"low\"\ngpu_priority = \"idle\"\npriority_boost = true\n",
    )
    .unwrap();
    let m = merge(&base, &all);
    assert_eq!(
        m.process
            .iter()
            .filter(|e| e.exe.eq_ignore_ascii_case("cg.exe"))
            .count(),
        1
    );
}

#[test]
fn a_boost_reads_on_or_off() {
    assert_eq!(boost_name(true), "on");
    assert_eq!(boost_name(false), "off");
}
