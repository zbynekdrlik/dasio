//! `windows_caller` on an `[[absent]]` exe (dasio#128): Edge's setup on
//! Windows 10 LTSC uninstalls only when its parent is `dllhost.exe` in
//! Windows' own folder (FOH 17.11.2026: "Parent process:
//! C:\Windows\SystemTemp\dllhost.exe", "Uninstall allowed: Windows caller").

use super::super::absent::{
    CALLER_NAME, CALLER_SOURCE, ExeStep, PRE_UNINSTALL_WRITES, Target, caller_command, caller_path,
    exe_steps,
};
use super::super::*;
use super::Pc;

const EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
const ALLOW: &str = r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstall";
const REFUSED: &str = "which cmd.exe reads itself (the Windows caller is cmd.exe)";

fn one(body: &str) -> Result<Profile, String> {
    parse(&format!("name = \"pc\"\n[[absent]]\n{body}\n"))
}

/// Edge's entry run through the Windows caller.
fn edge_caller(uninstall: &str) -> Result<Profile, String> {
    one(&format!(
        "exe = '{EDGE}'\nuninstall = [{uninstall}]\nbefore_uninstall = ['{ALLOW}']\nwindows_caller = true"
    ))
}

#[test]
fn the_windows_caller_is_a_cmd_copy_named_dllhost_in_system_temp() {
    assert_eq!(CALLER_SOURCE, r"System32\cmd.exe");
    assert_eq!(CALLER_NAME, "dllhost.exe");
    assert_eq!(
        caller_path(r"C:\Windows"),
        r"C:\Windows\SystemTemp\dllhost.exe"
    );
    assert_eq!(caller_path(r"D:\W"), r"D:\W\SystemTemp\dllhost.exe");
}

#[test]
fn the_callers_line_quotes_the_uninstaller_and_refuses_what_cmd_reads() {
    let setup =
        r"C:\Program Files (x86)\Microsoft\Edge\Application\156.0.4314.58\Installer\setup.exe";
    let args: Vec<String> = ["--uninstall", r"--log-file=C:\ProgramData\dasio\x.log"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        caller_command(setup, &args).unwrap(),
        format!(r#"/d /v:off /c ""{setup}" --uninstall --log-file=C:\ProgramData\dasio\x.log""#)
    );
    assert_eq!(
        caller_command(r"C:\s.exe", &[]).unwrap(),
        r#"/d /v:off /c ""C:\s.exe"""#
    );
    for c in ['"', '&', '|', '<', '>', '^', '%', '!', '\n', '\r'] {
        let bad = format!("a{c}b");
        let want = format!("{bad:?} holds {c:?}, {REFUSED}");
        assert_eq!(caller_command(&bad, &[]).unwrap_err(), want);
        assert_eq!(
            caller_command(r"C:\s.exe", &["--x".into(), bad.clone()]).unwrap_err(),
            want
        );
    }
    for bad in ["a b", "", "a\tb"] {
        assert_eq!(
            caller_command(r"C:\s.exe", &["--x".into(), bad.into()]).unwrap_err(),
            format!("{bad:?}: an argument through the Windows caller is one word, no spaces")
        );
    }
    // A space inside the uninstaller's own path is fine (it is quoted).
    assert!(caller_command(r"C:\a b\s.exe", &[]).is_ok());
}

#[test]
fn the_caller_is_copied_before_the_uninstaller_and_deleted_after_it() {
    let p = edge_caller(r"'C:\Program Files (x86)\s\*\setup.exe', '--uninstall'").unwrap();
    assert!(p.absent[0].windows_caller);
    let w = &PRE_UNINSTALL_WRITES[0];
    assert!(plan(&p, &Pc::default()).is_empty(), "Edge gone: nothing");
    let pc = Pc {
        files: vec![EDGE.into()],
        ..Pc::default()
    };
    let actions = plan(&p, &pc);
    assert_eq!(
        actions,
        vec![Action::Remove(Removal::Exe {
            exe: EDGE.into(),
            uninstall: vec![
                r"C:\Program Files (x86)\s\*\setup.exe".into(),
                "--uninstall".into()
            ],
            before: vec![w],
            windows_caller: true,
            fallback: None,
        })]
    );
    assert_eq!(
        describe(&actions[0]),
        format!(
            r#"the absence of {EDGE} (sets {ALLOW} = "", then runs C:\Program Files (x86)\s\*\setup.exe --uninstall through dllhost.exe (a copy of System32\cmd.exe))"#
        )
    );
    let args = ["--uninstall".to_string()];
    assert_eq!(
        exe_steps(&args, &[w], true),
        vec![
            ExeStep::Write(w),
            ExeStep::CopyCaller,
            ExeStep::Uninstall(args.to_vec()),
            ExeStep::DeleteCaller,
            ExeStep::Look
        ]
    );
    assert_eq!(
        exe_steps(&args, &[], true),
        vec![
            ExeStep::CopyCaller,
            ExeStep::Uninstall(args.to_vec()),
            ExeStep::DeleteCaller,
            ExeStep::Look
        ]
    );
    // Off by default.
    let plain = one(&format!("exe = '{EDGE}'\nuninstall = ['C:\\s.exe']")).unwrap();
    assert!(!plain.absent[0].windows_caller);
}

#[test]
fn a_windows_caller_entry_refuses_what_cmd_would_read() {
    assert_eq!(
        edge_caller(r"'C:\s.exe', '--a&b'").unwrap_err(),
        format!("absent exe {EDGE}: windows_caller: \"--a&b\" holds '&', {REFUSED}")
    );
    assert_eq!(
        edge_caller(r"'C:\s%x%.exe'").unwrap_err(),
        format!("absent exe {EDGE}: windows_caller: \"C:\\\\s%x%.exe\" holds '%', {REFUSED}")
    );
    assert_eq!(
        edge_caller(r"'C:\s.exe', 'a b'").unwrap_err(),
        format!(
            "absent exe {EDGE}: windows_caller: \"a b\": an argument through the Windows caller is one word, no spaces"
        )
    );
    // Without windows_caller the same arguments are the uninstaller's own.
    assert!(one(&format!("exe = '{EDGE}'\nuninstall = ['C:\\s.exe', 'a b']")).is_ok());
    for (body, what) in [
        (
            "service = \"edgeupdate\"\nwindows_caller = true",
            "absent service edgeupdate",
        ),
        ("task = '\\x*'\nwindows_caller = true", r"absent task \x*"),
    ] {
        assert_eq!(
            one(body).unwrap_err(),
            format!(
                "{what}: remove_dir, stop_processes, before_uninstall and windows_caller are for exe only"
            )
        );
    }
}

#[test]
fn the_baselines_edge_entry_runs_its_uninstaller_through_the_windows_caller() {
    let b = baseline().unwrap();
    let edge = b
        .absent
        .iter()
        .find(|e| e.target() == Some(Target::Exe(EDGE)))
        .unwrap();
    assert!(edge.windows_caller);
    // Only Edge's entry: EdgeUpdate's /uninstall needs none.
    assert_eq!(b.absent.iter().filter(|e| e.windows_caller).count(), 1);
    let cmd = edge.uninstall.as_ref().unwrap();
    assert_eq!(
        caller_command(&cmd[0], &cmd[1..]).unwrap(),
        r#"/d /v:off /c ""C:\Program Files (x86)\Microsoft\Edge\Application\*\Installer\setup.exe" --uninstall --msedge --system-level --verbose-logging --log-file=C:\ProgramData\dasio\msedge_installer.log --force-uninstall""#
    );
}
