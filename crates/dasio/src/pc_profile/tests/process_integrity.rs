//! A `[[process]]` entry's integrity level (dasio#128, 17.11.2026): both
//! FOH Lives, started from the elevated remote shell, ran at `high`, and
//! fohmixer's hub, at `medium`, got "SetWindowPos: Access is denied".

use super::super::*;
use super::{Pc, site};

const LIVE: &str = "Ableton Live 12 Suite.exe";

fn running(integrity: Result<String, String>) -> Pc {
    let mut pc = Pc::default();
    pc.procs.insert(
        LIVE.to_ascii_lowercase(),
        ProcState {
            affinity: 0xFFFF_0000,
            priority: "realtime".into(),
            integrity,
            extra: Extras::default(),
        },
    );
    pc
}

fn wants_medium() -> Profile {
    site(&format!(
        "[[process]]\nexe = \"{LIVE}\"\nintegrity = \"medium\"\nwhy = \"not elevated\"\n"
    ))
}

#[test]
fn integrity_parses_only_as_a_known_level() {
    for level in INTEGRITIES {
        let p = site(&format!(
            "[[process]]\nexe = \"a.exe\"\nintegrity = \"{level}\"\n"
        ));
        assert_eq!(p.process[0].integrity.as_deref(), Some(level));
    }
    for bad in ["elevated", "Medium", "medium plus", "untrusted", ""] {
        let text = format!("name = \"x\"\n[[process]]\nexe = \"a.exe\"\nintegrity = \"{bad}\"\n");
        let err = parse(&text).unwrap_err();
        assert!(
            err.contains("integrity") && err.contains("not one of"),
            "{bad}: {err}"
        );
    }
}

#[test]
fn the_label_rid_names_its_level() {
    assert_eq!(integrity_name(0), "untrusted");
    assert_eq!(integrity_name(0x0FFF), "untrusted");
    assert_eq!(integrity_name(0x1000), "low");
    assert_eq!(integrity_name(0x1FFF), "low");
    assert_eq!(integrity_name(0x2000), "medium");
    assert_eq!(integrity_name(0x20FF), "medium");
    assert_eq!(integrity_name(0x2100), "medium plus");
    assert_eq!(integrity_name(0x2FFF), "medium plus");
    assert_eq!(integrity_name(0x3000), "high");
    assert_eq!(integrity_name(0x3FFF), "high");
    assert_eq!(integrity_name(0x4000), "system");
    assert_eq!(integrity_name(0x4FFF), "system");
    assert_eq!(integrity_name(0x5000), "protected");
}

#[test]
fn a_daw_at_medium_is_no_problem() {
    let p = wants_medium();
    let pc = running(Ok("medium".into()));
    assert_eq!(check(&p, &pc), vec![]);
}

#[test]
fn an_elevated_daw_is_a_problem_and_nothing_is_applied() {
    let p = wants_medium();
    let pc = running(Ok("high".into()));
    let problems = check(&p, &pc);
    let text: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        [format!(
            "process {LIVE} integrity: high (the profile wants medium) — not elevated"
        )]
    );
    assert!(!problems[0].must);
    assert_eq!(plan(&p, &pc), vec![], "a running DAW is never de-elevated");
}

#[test]
fn an_unreadable_token_is_a_problem_not_a_pass() {
    let p = wants_medium();
    let pc = running(Err("OpenProcessToken: Access is denied.".into()));
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(
        problems[0].found,
        "unreadable (OpenProcessToken: Access is denied.)"
    );
}

#[test]
fn a_daw_not_running_is_no_finding() {
    assert_eq!(check(&wants_medium(), &Pc::default()), vec![]);
}

#[test]
fn integrity_is_unchecked_when_the_entry_does_not_name_it() {
    let p = site(&format!(
        "[[process]]\nexe = \"{LIVE}\"\naffinity = \"0xFFFF0000\"\n"
    ));
    assert_eq!(check(&p, &running(Ok("high".into()))), vec![]);
}

#[test]
fn a_site_affinity_entry_keeps_the_baseline_integrity_with_its_own_reason() {
    let base = parse_with(
        &format!(
            "name = \"b\"\n[[process]]\nexe = \"{LIVE}\"\nintegrity = \"medium\"\nwhy = \"base why\"\n"
        ),
        false,
    )
    .unwrap();
    let pc_profile = site(
        "[[process]]\nexe = \"ABLETON LIVE 12 SUITE.EXE\"\naffinity = \"0xFFFF0000\"\npriority = \"realtime\"\nwhy = \"site why\"\n",
    );
    let m = merge(&base, &pc_profile);
    assert_eq!(m.process.len(), 2, "{:#?}", m.process);
    let integrity: Vec<_> = m.process.iter().filter(|e| e.integrity.is_some()).collect();
    assert_eq!(integrity.len(), 1);
    assert_eq!(integrity[0].why, "base why");
    assert_eq!(integrity[0].affinity, None);
    assert_eq!(integrity[0].priority, None);
    let problems: Vec<String> = check(&m, &running(Ok("high".into())))
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        problems,
        [format!(
            "process {LIVE} integrity: high (the profile wants medium) — base why"
        )]
    );
}

#[test]
fn a_site_integrity_replaces_the_baseline_one() {
    let base = parse_with(
        &format!("name = \"b\"\n[[process]]\nexe = \"{LIVE}\"\nintegrity = \"medium\"\n"),
        false,
    )
    .unwrap();
    let m = merge(
        &base,
        &site(&format!(
            "[[process]]\nexe = \"{LIVE}\"\nintegrity = \"high\"\n"
        )),
    );
    assert_eq!(m.process.len(), 1);
    assert_eq!(m.process[0].integrity.as_deref(), Some("high"));
}

#[test]
fn the_baseline_holds_every_daw_at_medium() {
    let b = baseline().unwrap();
    for exe in [LIVE, "AbletonLiveMaster.exe"] {
        let e = b
            .process
            .iter()
            .find(|e| e.exe.eq_ignore_ascii_case(exe) && e.integrity.is_some())
            .unwrap_or_else(|| panic!("{exe} has no integrity entry"));
        assert_eq!(e.integrity.as_deref(), Some("medium"), "{exe}");
        assert!(e.why.contains("dasio#128"), "{exe}: {}", e.why);
    }
}

#[test]
fn a_site_entry_takes_over_exactly_the_fields_it_sets() {
    let base = parse_with(
        "name = \"b\"\n[[process]]\nexe = \"a.exe\"\naffinity = \"0x1\"\npriority = \"high\"\nintegrity = \"medium\"\nwhy = \"base\"\n",
        false,
    )
    .unwrap();
    let rest = |field: &str| {
        let m = merge(
            &base,
            &site(&format!("[[process]]\nexe = \"A.exe\"\n{field}\n")),
        );
        let b: Vec<_> = m.process.iter().filter(|e| e.why == "base").collect();
        assert_eq!(b.len(), 1, "{field}");
        (
            b[0].affinity.clone(),
            b[0].priority.clone(),
            b[0].integrity.clone(),
        )
    };
    let (a, p, i) = (
        Some("0x1".to_string()),
        Some("high".to_string()),
        Some("medium".to_string()),
    );
    assert_eq!(rest("affinity = \"0xF\""), (None, p.clone(), i.clone()));
    assert_eq!(rest("priority = \"idle\""), (a.clone(), None, i.clone()));
    assert_eq!(rest("integrity = \"high\""), (a.clone(), p.clone(), None));
    assert_eq!(rest("why = \"names nothing\""), (a, p, i));
}
