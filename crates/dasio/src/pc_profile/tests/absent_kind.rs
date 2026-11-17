//! `[[absent]]` entries (dasio#128, owner 16.11.2026): what must not exist
//! on a dasio PC, reported while it does and removed by `apply`.

use super::super::absent::{
    DELETABLE_SERVICES, ExeStep, Fallback, PRE_UNINSTALL_WRITES, REMOVABLE_DIRS, Step, Target,
    exe_steps, inside, parent_dir, pre_write, removable, resolve_uninstaller, split_star,
    task_folder, task_matches, versions_newest_first,
};
use super::super::*;
use super::Pc;

const EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
const UPDATE_DIR: &str = r"C:\Program Files (x86)\Microsoft\EdgeUpdate";
const UPDATE: &str = r"C:\Program Files (x86)\Microsoft\EdgeUpdate\MicrosoftEdgeUpdate.exe";

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
        ("task = '\\..\\..\\x*'", "as schtasks shows it"),
        ("task = '\\.\\x'", "as schtasks shows it"),
        ("task = '\\a\\\\x'", "as schtasks shows it"),
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
                before: vec![],
                fallback: None,
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
    assert_eq!(
        split_star(r"\*\b\setup.exe"),
        None,
        "no folder before the *"
    );
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
            "--verbose-logging",
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

/// EdgeUpdate's entry with its fallback, as the baseline gives it.
const UPDATE_ENTRY: &str = r"exe = 'C:\Program Files (x86)\Microsoft\EdgeUpdate\MicrosoftEdgeUpdate.exe'
uninstall = ['C:\Program Files (x86)\Microsoft\EdgeUpdate\MicrosoftEdgeUpdate.exe', '/uninstall']
remove_dir = 'C:\Program Files (x86)\Microsoft\EdgeUpdate'
stop_processes = ['MicrosoftEdgeUpdate.exe', 'MicrosoftEdgeUpdateCore.exe']";

#[test]
fn an_exe_entrys_fallback_parses_and_is_planned_after_its_uninstaller() {
    let p = one(UPDATE_ENTRY).unwrap();
    let e = &p.absent[0];
    assert_eq!(e.remove_dir.as_deref(), Some(UPDATE_DIR));
    let pc = Pc {
        files: vec![UPDATE.into()],
        ..Pc::default()
    };
    let actions = plan(&p, &pc);
    let fb = Fallback {
        dir: UPDATE_DIR.into(),
        stop: vec![
            "MicrosoftEdgeUpdate.exe".into(),
            "MicrosoftEdgeUpdateCore.exe".into(),
        ],
    };
    assert_eq!(
        actions,
        vec![Action::Remove(Removal::Exe {
            exe: UPDATE.into(),
            uninstall: vec![UPDATE.into(), "/uninstall".into()],
            before: vec![],
            fallback: Some(fb.clone()),
        })]
    );
    assert_eq!(
        describe(&actions[0]),
        format!(
            "the absence of {UPDATE} (runs {UPDATE} /uninstall; if that leaves the exe: stops MicrosoftEdgeUpdate.exe, MicrosoftEdgeUpdateCore.exe inside it, deletes {UPDATE_DIR})"
        )
    );
    assert_eq!(takes_effect(&actions[0]), "nothing more");
    // Every named process stopped first, in the entry's order, then the
    // folder deleted: a running exe keeps its folder.
    assert_eq!(
        fb.steps(),
        vec![
            Step::Stop {
                name: "MicrosoftEdgeUpdate.exe".into(),
                dir: UPDATE_DIR.into(),
            },
            Step::Stop {
                name: "MicrosoftEdgeUpdateCore.exe".into(),
                dir: UPDATE_DIR.into(),
            },
            Step::RemoveDir(UPDATE_DIR.into()),
        ]
    );
    // Without processes: only the folder; nothing found: nothing planned.
    let bare = one(&format!(
        "exe = '{UPDATE}'\nuninstall = ['{UPDATE}', '/uninstall']\nremove_dir = '{}'",
        UPDATE_DIR.to_ascii_uppercase()
    ))
    .unwrap();
    let Action::Remove(Removal::Exe {
        fallback: Some(f), ..
    }) = &plan(&bare, &pc)[0]
    else {
        panic!("no fallback planned");
    };
    assert_eq!(
        f.steps(),
        vec![Step::RemoveDir(UPDATE_DIR.to_ascii_uppercase())]
    );
    assert!(describe(&plan(&bare, &pc)[0]).ends_with(&format!(
        "; if that leaves the exe: deletes {})",
        UPDATE_DIR.to_ascii_uppercase()
    )));
    assert!(plan(&p, &Pc::default()).is_empty());
}

#[test]
fn a_fallback_deletes_only_the_allowed_folder_of_its_own_exe() {
    let exe_in = |dir: &str, extra: &str| {
        format!("exe = '{dir}\\x.exe'\nuninstall = ['C:\\u.exe']\n{extra}")
    };
    let web = r"C:\Program Files (x86)\Microsoft\EdgeWebView";
    let not_listed = |exe: &str, dir: &str| {
        format!(
            "absent exe {exe}: dasio deletes only the folders {REMOVABLE_DIRS:?} (pc_profile::absent::REMOVABLE_DIRS), not {dir}"
        )
    };
    let not_own = |exe: &str| {
        format!("absent exe {exe}: remove_dir {UPDATE_DIR} must be the exe's own folder")
    };
    let sub = format!(r"{UPDATE_DIR}\sub\x.exe");
    let refused = [
        // Not on the list, even as the exe's own folder.
        (
            exe_in(web, &format!("remove_dir = '{web}'")),
            not_listed(&format!(r"{web}\x.exe"), web),
        ),
        (
            exe_in(r"C:\Windows", "remove_dir = 'C:\\Windows'"),
            not_listed(r"C:\Windows\x.exe", r"C:\Windows"),
        ),
        // On the list, but not the exe's folder.
        (
            format!("exe = '{EDGE}'\nuninstall = ['C:\\u.exe']\nremove_dir = '{UPDATE_DIR}'"),
            not_own(EDGE),
        ),
        (
            format!("exe = '{sub}'\nuninstall = ['C:\\u.exe']\nremove_dir = '{UPDATE_DIR}'"),
            not_own(&sub),
        ),
        (
            format!(
                "exe = '{UPDATE}'\nuninstall = ['C:\\u.exe']\nstop_processes = ['MicrosoftEdgeUpdate.exe']"
            ),
            format!(
                "absent exe {UPDATE}: stop_processes needs remove_dir (they stop only what runs from it)"
            ),
        ),
        (
            "service = \"edgeupdate\"\nremove_dir = 'C:\\x'".to_string(),
            "absent service edgeupdate: remove_dir, stop_processes and before_uninstall are for exe only".to_string(),
        ),
        (
            "task = '\\x*'\nstop_processes = ['a.exe']".to_string(),
            r"absent task \x*: remove_dir, stop_processes and before_uninstall are for exe only".to_string(),
        ),
    ];
    for (body, want) in refused {
        assert_eq!(one(&body).unwrap_err(), want, "{body:?}");
    }
    for bad in [
        r"C:\x\a.exe",
        "x/a.exe",
        "a*.exe",
        "C:a.exe",
        "a.dll",
        ".exe",
        "",
    ] {
        let body = format!(
            "exe = '{UPDATE}'\nuninstall = ['C:\\u.exe']\nremove_dir = '{UPDATE_DIR}'\nstop_processes = ['ok.exe', '{bad}']"
        );
        let e = one(&body).unwrap_err();
        assert_eq!(
            e,
            format!(
                "absent exe {UPDATE}: stop_processes {bad:?}: give an exe file name, e.g. MicrosoftEdgeUpdate.exe, no folder, no *"
            ),
            "{bad:?}"
        );
    }
    assert!(one(&format!(
        "exe = '{UPDATE}'\nuninstall = ['C:\\u.exe']\nremove_dir = '{UPDATE_DIR}'\nstop_processes = ['A.EXE', 'b.exe']"
    ))
    .is_ok());
    assert_eq!(
        REMOVABLE_DIRS,
        [UPDATE_DIR],
        "widening the list deletes more than the baseline names: review it"
    );
    assert!(removable(UPDATE_DIR));
    assert!(removable(&UPDATE_DIR.to_ascii_lowercase()));
    assert!(!removable(r"C:\Program Files (x86)\Microsoft"));
    assert!(!removable(r"C:\Program Files (x86)\Microsoft\EdgeUpdate\"));
    assert_eq!(parent_dir(UPDATE), Some(UPDATE_DIR));
    assert_eq!(parent_dir("x.exe"), None);
}

#[test]
fn only_a_process_running_from_inside_the_folder_is_stopped() {
    assert!(inside(UPDATE_DIR, UPDATE));
    assert!(inside(UPDATE_DIR, &UPDATE.to_ascii_uppercase()));
    assert!(inside(
        UPDATE_DIR,
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\1.3.195.61\MicrosoftEdgeUpdateCore.exe"
    ));
    for outside in [
        UPDATE_DIR,
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\",
        r"C:\Program Files (x86)\Microsoft\EdgeUpdateX\MicrosoftEdgeUpdate.exe",
        r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\MicrosoftEdgeUpdate.exe",
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\..\EdgeWebView\a.exe",
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\.\a.exe",
        r"C:\Program Files (x86)\Microsoft\EdgeUpdate\\a.exe",
        r"C:\Program Files (x86)\Microsoft\Edge",
        r"D:\Program Files (x86)\Microsoft\EdgeUpdate\a.exe",
        "",
    ] {
        assert!(!inside(UPDATE_DIR, outside), "{outside}");
    }
}

#[test]
fn the_baselines_edgeupdate_entry_deletes_its_folder_when_it_stays() {
    let b = baseline().unwrap();
    let e = b
        .absent
        .iter()
        .find(|e| e.target() == Some(Target::Exe(UPDATE)))
        .unwrap();
    assert_eq!(e.remove_dir.as_deref(), Some(UPDATE_DIR));
    assert_eq!(
        e.stop_processes.as_deref(),
        Some(
            [
                "MicrosoftEdgeUpdate.exe",
                "MicrosoftEdgeUpdateCore.exe",
                "MicrosoftEdgeUpdateOnDemand.exe",
                "MicrosoftEdgeUpdateBroker.exe"
            ]
            .map(String::from)
            .as_slice()
        )
    );
    // Edge's own entry has no fallback: WebView2 shares its parent.
    let edge = b
        .absent
        .iter()
        .find(|e| e.target() == Some(Target::Exe(EDGE)))
        .unwrap();
    assert_eq!(edge.remove_dir, None);
}

/// EdgeUpdateDev's `AllowUninstall`, as an entry names it.
const ALLOW: &str = r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstall";

/// Edge's entry with the write before its uninstaller.
fn edge_with(before: &str) -> Result<Profile, String> {
    one(&format!(
        "exe = '{EDGE}'\nuninstall = ['C:\\setup.exe', '--uninstall']\nbefore_uninstall = [{before}]"
    ))
}

#[test]
fn only_edgeupdatedevs_allowuninstall_is_written_before_an_uninstaller() {
    // The list is in code: an empty REG_SZ, as the known removers write it
    // (FOH 17.11.2026, Edge refused on Windows 10 LTSC).
    assert_eq!(
        PRE_UNINSTALL_WRITES.len(),
        1,
        "widening it writes more: review it"
    );
    let w = &PRE_UNINSTALL_WRITES[0];
    assert_eq!(w.key, r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev");
    assert_eq!(w.name, "AllowUninstall");
    assert_eq!(w.data, "");
    assert_eq!(w.path(), ALLOW);
    assert_eq!(pre_write(ALLOW), Some(w));
    assert_eq!(
        pre_write(&ALLOW.to_ascii_lowercase()),
        Some(w),
        "case ignored"
    );
    for other in [
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstal",
        r"HKLM\SOFTWARE\Microsoft\EdgeUpdateDev\AllowUninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstall\x",
        "",
    ] {
        assert_eq!(pre_write(other), None, "{other}");
    }
    let p = edge_with(&format!("'{}'", ALLOW.to_ascii_uppercase())).unwrap();
    assert_eq!(
        p.absent[0].before_uninstall.as_deref(),
        Some([ALLOW.to_ascii_uppercase()].as_slice())
    );
    let bad = r"HKLM\SYSTEM\CurrentControlSet\Services\x\Start";
    assert_eq!(
        edge_with(&format!("'{ALLOW}', '{bad}'")).unwrap_err(),
        format!(
            "absent exe {EDGE}: before_uninstall {bad}: dasio writes only [{ALLOW:?}] (pc_profile::absent::PRE_UNINSTALL_WRITES)"
        )
    );
    for body in [
        format!("service = \"edgeupdate\"\nbefore_uninstall = ['{ALLOW}']"),
        format!("task = '\\x*'\nbefore_uninstall = ['{ALLOW}']"),
    ] {
        let err = one(&body).unwrap_err();
        assert!(
            err.ends_with(": remove_dir, stop_processes and before_uninstall are for exe only"),
            "{err}"
        );
    }
}

#[test]
fn the_write_comes_before_the_uninstaller_and_only_while_the_exe_is_there() {
    let p = edge_with(&format!("'{ALLOW}'")).unwrap();
    let w = &PRE_UNINSTALL_WRITES[0];
    // Edge gone: nothing planned, so nothing is written.
    assert!(plan(&p, &Pc::default()).is_empty());
    let pc = Pc {
        files: vec![EDGE.into()],
        ..Pc::default()
    };
    let actions = plan(&p, &pc);
    assert_eq!(
        actions,
        vec![Action::Remove(Removal::Exe {
            exe: EDGE.into(),
            uninstall: vec![r"C:\setup.exe".into(), "--uninstall".into()],
            before: vec![w],
            fallback: None,
        })]
    );
    assert_eq!(
        describe(&actions[0]),
        format!(r#"the absence of {EDGE} (sets {ALLOW} = "", then runs C:\setup.exe --uninstall)"#)
    );
    let args = ["--uninstall".to_string(), "--msedge".to_string()];
    assert_eq!(
        exe_steps(&args, &[w]),
        vec![
            ExeStep::Write(w),
            ExeStep::Uninstall(args.to_vec()),
            ExeStep::Look
        ]
    );
    assert_eq!(
        exe_steps(&args, &[]),
        vec![ExeStep::Uninstall(args.to_vec()), ExeStep::Look]
    );
}

#[test]
fn the_baselines_edge_entry_allows_its_uninstall_first() {
    let b = baseline().unwrap();
    let edge = b
        .absent
        .iter()
        .find(|e| e.target() == Some(Target::Exe(EDGE)))
        .unwrap();
    assert_eq!(
        edge.before_uninstall.as_deref(),
        Some([ALLOW.to_string()].as_slice())
    );
    // Only Edge's entry writes anything first.
    assert_eq!(
        b.absent
            .iter()
            .filter(|e| e.before_uninstall.is_some())
            .count(),
        1
    );
}
