//! `[[absent]]` entries (dasio#128, owner 16.11.2026): what must not exist
//! on a dasio PC, reported while it does and removed by `apply`.

use super::super::absent::{
    DELETABLE_SERVICES, Target, resolve_uninstaller, split_star, task_folder, task_matches,
    versions_newest_first,
};
use super::super::*;
use super::Pc;

const EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";

const ABSENT: &str = r#"
name = "pc"

[[absent]]
exe = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
uninstall = ['C:\Program Files (x86)\Microsoft\Edge\Application\*\Installer\setup.exe', '--uninstall', '--msedge', '--system-level', '--force-uninstall']
why = "no browser on a dasio PC"

[[absent]]
service = "edgeupdate"
why = "no updater"

[[absent]]
task = '\MicrosoftEdgeUpdateTaskMachineCore*'
why = "no updater task"
"#;

fn profile() -> Profile {
    parse(ABSENT).unwrap()
}

/// A profile of one `[[absent]]` entry given as its TOML body.
fn one(body: &str) -> Result<Profile, String> {
    parse(&format!("name = \"pc\"\n[[absent]]\n{body}\n"))
}

#[test]
fn an_absent_entry_parses_with_its_target() {
    let p = profile();
    assert_eq!(p.absent.len(), 3);
    assert_eq!(p.entries(), 3, "absent entries are counted");
    assert_eq!(p.absent[0].target(), Some(Target::Exe(EDGE)));
    assert_eq!(p.absent[0].uninstall.as_ref().unwrap().len(), 5);
    assert_eq!(p.absent[1].target(), Some(Target::Service("edgeupdate")));
    assert_eq!(
        p.absent[2].target(),
        Some(Target::Task(r"\MicrosoftEdgeUpdateTaskMachineCore*"))
    );
    assert_eq!(p.absent[1].what(), "absent service edgeupdate");
}

#[test]
fn an_absent_entry_that_names_too_little_or_too_much_is_refused() {
    let refused = [
        ("why = \"x\"", "exactly one of exe, service, task"),
        (
            "service = \"edgeupdate\"\ntask = '\\x'",
            "exactly one of exe, service, task",
        ),
        ("exe = 'msedge.exe'\nuninstall = ['C:\\u.exe']", "full path"),
        ("exe = 'C:\\'\nuninstall = ['C:\\u.exe']", "full path"),
        (
            "exe = 'C:\\a\\*\\x.exe'\nuninstall = ['C:\\u.exe']",
            "full path",
        ),
        ("exe = 'C:\\a\\x.exe'", "give uninstall"),
        ("exe = 'C:\\a\\x.exe'\nuninstall = []", "give uninstall"),
        (
            "exe = 'C:\\a\\x.exe'\nuninstall = ['u.exe']",
            "needs its full path",
        ),
        (
            "exe = 'C:\\a\\x.exe'\nuninstall = ['C:\\a\\*\\b\\*\\u.exe']",
            "one * as a whole folder",
        ),
        (
            "exe = 'C:\\a\\x.exe'\nuninstall = ['C:\\a\\v*\\u.exe']",
            "one * as a whole folder",
        ),
        ("service = \"WinDefend\"", "dasio deletes only"),
        (
            "service = \"edgeupdate\"\nuninstall = ['C:\\u.exe']",
            "uninstall is for exe only",
        ),
        ("task = 'NoSlash'", "as schtasks shows it"),
        ("task = '\\*'", "as schtasks shows it"),
        ("task = '\\a*b'", "as schtasks shows it"),
        ("task = '\\a\\'", "as schtasks shows it"),
        (
            "task = '\\a'\nuninstall = ['C:\\u.exe']",
            "uninstall is for exe only",
        ),
        ("service = \"edgeupdate\"\ncommand = \"x\"", "unknown field"),
    ];
    for (body, want) in refused {
        let e = one(body).unwrap_err();
        assert!(e.contains(want), "{body:?}: {e}");
    }
}

#[test]
fn every_deletable_service_parses_case_aside() {
    for s in DELETABLE_SERVICES {
        assert!(one(&format!("service = \"{s}\"")).is_ok(), "{s}");
        assert!(
            one(&format!("service = \"{}\"", s.to_ascii_uppercase())).is_ok(),
            "{s}"
        );
    }
    assert_eq!(
        DELETABLE_SERVICES,
        ["edgeupdate", "edgeupdatem", "MicrosoftEdgeElevationService"],
        "widening the list deletes more than the baseline names: review it"
    );
}

#[test]
fn nothing_present_is_no_problem_and_no_removal() {
    let pc = Pc::default();
    assert!(check(&profile(), &pc).is_empty());
    assert!(plan(&profile(), &pc).is_empty());
}

#[test]
fn what_exists_is_a_problem_and_its_removal_is_planned() {
    let pc = Pc {
        files: vec![EDGE.to_ascii_lowercase()],
        services: vec!["EdgeUpdate".into()],
        task_names: vec![
            r"\MicrosoftEdgeUpdateTaskMachineCore{A4CF2E7C-0000-0000-0000-000000000000}".into(),
            r"\MicrosoftEdgeUpdateTaskMachineUA{5F64A35B-0000-0000-0000-000000000000}".into(),
            r"\Other\MicrosoftEdgeUpdateTaskMachineCore{1}".into(),
            r"\fleetbackup-main".into(),
        ],
        ..Pc::default()
    };
    let texts: Vec<String> = check(&profile(), &pc)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(texts.len(), 3, "{texts:#?}");
    assert!(texts[0].starts_with(&format!("absent exe {EDGE}: present (")));
    assert!(texts[0].contains("(the profile wants absent) — no browser on a dasio PC"));
    assert!(texts[1].starts_with("absent service edgeupdate: present (edgeupdate)"));
    assert!(
        texts[2].contains(
            r"present (\MicrosoftEdgeUpdateTaskMachineCore{A4CF2E7C-0000-0000-0000-000000000000})"
        ),
        "only the root folder's Core task: {}",
        texts[2]
    );
    assert!(check(&profile(), &pc).iter().all(|p| !p.must));
    let actions = plan(&profile(), &pc);
    assert_eq!(
        actions,
        vec![
            Action::Remove(Removal::Exe {
                exe: EDGE.into(),
                uninstall: vec![
                    r"C:\Program Files (x86)\Microsoft\Edge\Application\*\Installer\setup.exe"
                        .into(),
                    "--uninstall".into(),
                    "--msedge".into(),
                    "--system-level".into(),
                    "--force-uninstall".into(),
                ],
            }),
            Action::Remove(Removal::Service("edgeupdate".into())),
            Action::Remove(Removal::Task(
                r"\MicrosoftEdgeUpdateTaskMachineCore{A4CF2E7C-0000-0000-0000-000000000000}".into()
            )),
        ]
    );
    assert_eq!(
        describe(&actions[1]),
        "the absence of service edgeupdate (sc stop + sc delete)"
    );
    assert!(describe(&actions[0]).contains("(runs C:\\Program Files (x86)\\Microsoft\\Edge"));
    assert_eq!(
        describe(&actions[2]),
        r"the absence of task \MicrosoftEdgeUpdateTaskMachineCore{A4CF2E7C-0000-0000-0000-000000000000} (schtasks /delete)"
    );
    assert!(takes_effect(&actions[1]).contains("reboot"));
    assert_eq!(takes_effect(&actions[2]), "nothing more");
}

#[test]
fn an_unreadable_pc_is_a_problem_and_removes_nothing() {
    let pc = Pc {
        absent_err: Some("access denied".into()),
        ..Pc::default()
    };
    let problems = check(&profile(), &pc);
    assert_eq!(problems.len(), 3);
    assert_eq!(problems[1].found, "unreadable (access denied)");
    assert!(plan(&profile(), &pc).is_empty());
}

#[test]
fn a_task_pattern_matches_its_folder_only() {
    let p = r"\MicrosoftEdgeUpdateTaskMachineCore*";
    assert!(task_matches(
        p,
        r"\MicrosoftEdgeUpdateTaskMachineCore{F5F2}"
    ));
    assert!(task_matches(
        p,
        r"\microsoftedgeupdatetaskmachinecore{f5f2}"
    ));
    assert!(!task_matches(p, r"\MicrosoftEdgeUpdateTaskMachineCore"));
    assert!(!task_matches(p, r"\MicrosoftEdgeUpdateTaskMachineCore\x"));
    assert!(!task_matches(p, r"\MicrosoftEdgeUpdateTaskMachineUA{1}"));
    assert!(!task_matches(
        p,
        r"\Sub\MicrosoftEdgeUpdateTaskMachineCore{1}"
    ));
    assert!(task_matches(r"\dasio-x", r"\DASIO-X"));
    assert!(!task_matches(r"\dasio-x", r"\dasio-xy"));
    assert_eq!(task_folder(p), "");
    assert_eq!(
        task_folder(r"\Microsoft\Windows\Defender*"),
        r"Microsoft\Windows"
    );
}

#[test]
fn the_uninstaller_runs_from_the_newest_version_folder() {
    assert_eq!(
        split_star(r"C:\a\*\b\setup.exe"),
        Some((r"C:\a", r"b\setup.exe"))
    );
    assert_eq!(split_star(r"C:\a\b\setup.exe"), None);
    assert_eq!(split_star(r"C:\a\v*\setup.exe"), None);
    assert_eq!(split_star(r"\*\b\setup.exe"), None, "no folder before the *");
    assert_eq!(split_star(r"C:\a\*\"), None, "nothing after the *");
    let names: Vec<String> = [
        "154.0.4258.62.deleting",
        "156.0.4314.50",
        "SetupMetrics",
        "156.0.4314.58",
        "9.0.0.1",
        "10.0.0.1",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        versions_newest_first(&names),
        ["156.0.4314.58", "156.0.4314.50", "10.0.0.1", "9.0.0.1"]
    );
    let list = |dir: &str| {
        assert_eq!(dir, r"C:\Edge");
        names.clone()
    };
    // The newest whose uninstaller is there.
    let exists = |p: &str| p == r"C:\Edge\156.0.4314.50\Installer\setup.exe";
    assert_eq!(
        resolve_uninstaller(r"C:\Edge\*\Installer\setup.exe", list, exists).as_deref(),
        Some(r"C:\Edge\156.0.4314.50\Installer\setup.exe")
    );
    assert_eq!(
        resolve_uninstaller(r"C:\Edge\*\Installer\setup.exe", list, |_| false),
        None
    );
    assert_eq!(
        resolve_uninstaller(r"C:\EdgeUpdate\MicrosoftEdgeUpdate.exe", list, |_| false).as_deref(),
        Some(r"C:\EdgeUpdate\MicrosoftEdgeUpdate.exe"),
        "no * is run as given (its absence is the run's error)"
    );
}

#[test]
fn a_site_absent_entry_replaces_the_baselines_same_target() {
    let base = profile();
    let site = parse(
        "name = \"pc\"\n[[absent]]\nservice = \"EDGEUPDATE\"\nwhy = \"site\"\n[[absent]]\nservice = \"edgeupdatem\"\n",
    )
    .unwrap();
    let m = merge(&base, &site);
    assert_eq!(m.absent.len(), 4, "{:#?}", m.absent);
    let upd: Vec<_> = m
        .absent
        .iter()
        .filter(|e| {
            e.service
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("edgeupdate"))
        })
        .collect();
    assert_eq!(upd.len(), 1);
    assert_eq!(upd[0].why, "site");
    assert_eq!(baseline_only(&base).absent.len(), 3);
}

#[test]
fn the_bridges_in_process_check_keeps_the_absences() {
    // Absences are read from the registry and the file system, no process
    // is started (dasio#106): the bridge checks them in process too.
    assert_eq!(in_process(&profile()).absent.len(), 3);
}

#[test]
fn the_baseline_keeps_edge_and_its_updater_off_every_pc() {
    let b = baseline().unwrap();
    let has = |t: Target| b.absent.iter().any(|e| e.target() == Some(t));
    assert!(has(Target::Exe(EDGE)));
    assert!(has(Target::Exe(
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\MicrosoftEdgeUpdate.exe"
    )));
    for s in DELETABLE_SERVICES {
        assert!(has(Target::Service(s)), "{s}");
    }
    assert!(has(Target::Task(r"\MicrosoftEdgeUpdateTaskMachineCore*")));
    assert!(has(Target::Task(r"\MicrosoftEdgeUpdateTaskMachineUA*")));
    let edge = b
        .absent
        .iter()
        .find(|e| e.target() == Some(Target::Exe(EDGE)))
        .unwrap();
    assert_eq!(
        edge.uninstall.as_deref().unwrap()[1..],
        [
            "--uninstall",
            "--msedge",
            "--system-level",
            "--force-uninstall"
        ]
    );
    assert!(b.absent.iter().all(|e| !e.why.is_empty()));
}

#[test]
fn an_unreadable_ini_file_is_one_problem_and_one_write() {
    // A PC without Process Lasso: one problem for its ini, not one per key
    // (the baseline holds ~150 of them, dasio#128).
    let p = parse(
        "name = \"pc\"\n[[ini]]\nfile = 'C:\\L\\p.ini'\nkey = \"A\"\nvalue = \"1\"\n[[ini]]\nfile = 'C:\\L\\p.ini'\nkey = \"B\"\nvalue = \"2\"\n[[ini]]\nfile = 'C:\\L\\p.ini'\nkey = \"C\"\nvalue = \"3\"\n[[ini]]\nfile = 'C:\\M\\q.ini'\nkey = \"A\"\nvalue = \"1\"\n",
    )
    .unwrap();
    let pc = Pc {
        ini_err: vec![r"C:\L\p.ini".into()],
        ..Pc::default()
    };
    let texts: Vec<String> = check(&p, &pc).iter().map(ToString::to_string).collect();
    assert_eq!(
        texts,
        [
            r"C:\L\p.ini: unreadable (C:\L\p.ini: not found) (the profile wants the file, with the 3 key(s) the profile names)",
            r#"C:\M\q.ini A: absent (the profile wants "1")"#,
        ]
    );
    let writes: Vec<_> = plan(&p, &pc)
        .into_iter()
        .filter_map(|a| match a {
            Action::Ini { file, key, .. } => Some(format!("{file} {key}")),
            _ => None,
        })
        .collect();
    assert_eq!(writes, [r"C:\L\p.ini A", r"C:\M\q.ini A"]);
}
