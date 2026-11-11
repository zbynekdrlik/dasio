use std::collections::HashMap;

use super::*;

/// A PC as a test sets it up: the reader the check sees.
#[derive(Default)]
struct Pc {
    reg: HashMap<(String, String), Value>,
    unreadable: Vec<String>,
    ini: HashMap<(String, String), String>,
    procs: HashMap<String, ProcState>,
    free: HashMap<String, u64>,
    bcd: Vec<(String, String)>,
    /// `bcd()` fails with this.
    bcd_err: Option<String>,
    /// How often `bcd()` was called.
    bcd_reads: std::cell::Cell<u32>,
    tasks: HashMap<String, TaskState>,
    /// `task()` fails for these names.
    task_err: Vec<String>,
}

impl Reader for Pc {
    fn reg(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
        if self.unreadable.iter().any(|k| k == key) {
            return Err("access denied".into());
        }
        Ok(self.reg.get(&(key.to_string(), name.to_string())).cloned())
    }
    fn ini(&self, file: &str, key: &str) -> Result<Option<String>, String> {
        Ok(self.ini.get(&(file.to_string(), key.to_string())).cloned())
    }
    fn process(&self, exe: &str) -> Option<ProcState> {
        self.procs.get(&exe.to_ascii_lowercase()).cloned()
    }
    fn free_bytes(&self, drive: &str) -> Result<u64, String> {
        if self.unreadable.iter().any(|k| k == drive) {
            return Err("the device is not ready".into());
        }
        self.free
            .get(drive)
            .copied()
            .ok_or_else(|| "no such drive".into())
    }
    fn task(&self, name: &str) -> Result<Option<TaskState>, String> {
        if self.task_err.iter().any(|n| n == name) {
            return Err("access denied".into());
        }
        Ok(self.tasks.get(name).cloned())
    }
    fn bcd(&self) -> Result<Vec<(String, String)>, String> {
        self.bcd_reads.set(self.bcd_reads.get() + 1);
        match &self.bcd_err {
            Some(e) => Err(e.clone()),
            None => Ok(self.bcd.clone()),
        }
    }
}

const PROFILE: &str = r#"
name = "FOH"
nic_guid = "{65CC9138-0000-0000-0000-000000000000}"

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\MMCSS'
name = "Start"
dword = 4
why = "MMCSS demotes the DAWs' audio threads"

[[registry]]
key = '{nic_class}'
name = "*InterruptModeration"
string = "0"

[[registry]]
key = '{nic_device}\Interrupt Management\Affinity Policy'
name = "AssignmentSetOverride"
binary = "0f00000000000000"

[[ini]]
file = 'C:\lasso.ini'
key = "DefaultAffinitiesEx"
value = "abletonlivemaster.exe,0,4-7"
why = "master off band's CCD"

[[process]]
exe = "AbletonLiveMaster.exe"
affinity = "0xF0"
priority = "high"

[[disk]]
drive = "C:"
min_free_gb = 50
why = "a full C: stalls the DAWs"
"#;

fn matching_pc() -> Pc {
    let mut pc = Pc::default();
    pc.reg.insert(
        (
            r"HKLM\SYSTEM\CurrentControlSet\Services\MMCSS".into(),
            "Start".into(),
        ),
        Value::Dword(4),
    );
    pc.reg.insert(
        ("{nic_class}".into(), "*InterruptModeration".into()),
        Value::String("0".into()),
    );
    pc.reg.insert(
        (
            r"{nic_device}\Interrupt Management\Affinity Policy".into(),
            "AssignmentSetOverride".into(),
        ),
        Value::Binary(vec![0x0f, 0, 0, 0, 0, 0, 0, 0]),
    );
    pc.ini.insert(
        (r"C:\lasso.ini".into(), "DefaultAffinitiesEx".into()),
        "abletonlivemaster.exe,0,4-7".into(),
    );
    pc.procs.insert(
        "abletonlivemaster.exe".into(),
        ProcState {
            affinity: 0xF0,
            priority: "high".into(),
        },
    );
    pc.free.insert("C:".into(), 100 << 30);
    pc
}

#[test]
fn a_pc_as_the_profile_says_has_no_problem_and_nothing_to_apply() {
    let p = parse(PROFILE).unwrap();
    assert_eq!(p.name, "FOH");
    let pc = matching_pc();
    assert_eq!(check(&p, &pc), vec![]);
    assert_eq!(plan(&p, &pc), vec![]);
}

#[test]
fn every_kind_of_difference_is_a_problem_with_its_reason() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    // A Windows update turned MMCSS back on, Lasso's affinity was edited,
    // the NIC lost its value, and Lasso did not apply its rule.
    pc.reg.insert(
        (
            r"HKLM\SYSTEM\CurrentControlSet\Services\MMCSS".into(),
            "Start".into(),
        ),
        Value::Dword(2),
    );
    pc.reg
        .remove(&("{nic_class}".into(), "*InterruptModeration".into()));
    pc.ini.insert(
        (r"C:\lasso.ini".into(), "DefaultAffinitiesEx".into()),
        "abletonlivemaster.exe,0,26-31".into(),
    );
    pc.procs.insert(
        "abletonlivemaster.exe".into(),
        ProcState {
            affinity: 0xFC00_0000,
            priority: "realtime".into(),
        },
    );
    let problems = check(&p, &pc);
    let text: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert_eq!(problems.len(), 5, "{text:#?}");
    assert!(text[0].contains(r"Services\MMCSS\Start: 2 (0x2) (the profile wants 4 (0x4))"));
    assert!(text[0].ends_with("— MMCSS demotes the DAWs' audio threads"));
    assert!(text[1].contains("*InterruptModeration: absent"));
    assert!(text[2].contains("DefaultAffinitiesEx: \"abletonlivemaster.exe,0,26-31\""));
    assert!(text[2].ends_with("— master off band's CCD"));
    assert!(text[3].contains("affinity: 0xFC000000 (the profile wants 0xF0)"));
    assert!(text[4].contains("priority: realtime (the profile wants high)"));
    // Only the registry and the ini are applied; the process follows Lasso.
    assert_eq!(
        plan(&p, &pc),
        vec![
            Action::Reg {
                key: r"HKLM\SYSTEM\CurrentControlSet\Services\MMCSS".into(),
                name: "Start".into(),
                value: Value::Dword(4),
            },
            Action::Reg {
                key: "{nic_class}".into(),
                name: "*InterruptModeration".into(),
                value: Value::String("0".into()),
            },
            Action::Ini {
                file: r"C:\lasso.ini".into(),
                key: "DefaultAffinitiesEx".into(),
                value: "abletonlivemaster.exe,0,4-7".into(),
            },
        ]
    );
}

#[test]
fn an_unreadable_value_is_a_problem_and_a_process_not_running_is_none() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    pc.unreadable.push("{nic_class}".into());
    pc.procs.clear();
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].found, "unreadable (access denied)");
    assert_eq!(
        plan(&p, &pc).len(),
        1,
        "planned: the write says why it fails"
    );
}

#[test]
fn a_wrong_binary_value_shows_both_as_hex() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    pc.reg.insert(
        (
            r"{nic_device}\Interrupt Management\Affinity Policy".into(),
            "AssignmentSetOverride".into(),
        ),
        Value::Binary(vec![0xff, 0, 0, 0, 0, 0, 0, 0]),
    );
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].found, "ff00000000000000");
    assert_eq!(problems[0].want, "0f00000000000000");
}

#[test]
fn a_profile_with_a_mistake_is_refused_with_what_is_wrong() {
    let two =
        "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\ndword = 1\nstring = \"1\"\n";
    assert!(
        parse(two)
            .unwrap_err()
            .contains("exactly one of dword, string, binary")
    );
    let none = "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\n";
    assert!(parse(none).is_err());
    let odd = "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\nbinary = \"0f0\"\n";
    assert!(parse(odd).is_err());
    let nic = "name = \"x\"\n[[registry]]\nkey = '{nic_class}'\nname = \"B\"\ndword = 1\n";
    assert!(parse(nic).unwrap_err().contains("needs nic_guid"));
    let mask = "name = \"x\"\n[[process]]\nexe = \"a.exe\"\naffinity = \"zz\"\n";
    assert!(parse(mask).unwrap_err().contains("not a hex mask"));
    let class = "name = \"x\"\n[[process]]\nexe = \"a.exe\"\npriority = \"turbo\"\n";
    assert!(parse(class).unwrap_err().contains("not one of"));
    let typo = "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nnmae = \"B\"\ndword = 1\n";
    assert!(parse(typo).is_err(), "unknown fields are refused");
}

#[test]
fn masks_read_with_or_without_0x() {
    assert_eq!(parse_mask("0xF0"), Some(0xF0));
    assert_eq!(parse_mask("FC000000"), Some(0xFC00_0000));
    assert_eq!(parse_mask(" 0X3ff0000 "), Some(0x3FF_0000));
    assert_eq!(parse_mask("g"), None);
}

#[test]
fn what_each_change_needs_before_it_takes_effect() {
    let reg = |key: &str, name: &str| Action::Reg {
        key: key.into(),
        name: name.into(),
        value: Value::Dword(0),
    };
    assert_eq!(
        takes_effect(&reg("{nic_class}", "*RSS")),
        "a restart of the Dante NIC"
    );
    assert_eq!(
        takes_effect(&reg(
            r"HKLM\SYSTEM\CurrentControlSet\Services\MMCSS",
            "Start"
        )),
        "a reboot"
    );
    assert!(
        takes_effect(&reg(
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile",
            "SystemResponsiveness"
        ))
        .contains("MMCSS")
    );
    assert!(
        takes_effect(&reg(
            r"HKLM\SYSTEM\CurrentControlSet\Services\dasiopacer\Parameters",
            "RxdCpu"
        ))
        .contains("filter")
    );
    assert_eq!(
        takes_effect(&reg(r"HKLM\SOFTWARE\Policies\X", "Y")),
        "nothing more"
    );
    assert!(
        takes_effect(&Action::Ini {
            file: "f".into(),
            key: "k".into(),
            value: "v".into()
        })
        .contains("Lasso")
    );
}

#[test]
fn an_ini_line_is_set_in_place_and_a_missing_one_added() {
    let text = "[A]\r\nX=1\r\nDefaultAffinitiesEx=old\r\nY=2\r\n";
    let set = set_ini(text, "DefaultAffinitiesEx", "new,0,4-7");
    assert_eq!(
        set,
        "[A]\r\nX=1\r\nDefaultAffinitiesEx=new,0,4-7\r\nY=2\r\n"
    );
    assert_eq!(
        get_ini(&set, "DefaultAffinitiesEx").as_deref(),
        Some("new,0,4-7")
    );
    // Only the first line of a key, and not a key that merely starts alike.
    let two = "XX=0\nX=1\nX=2\n";
    assert_eq!(set_ini(two, "X", "9"), "XX=0\nX=9\nX=2\n");
    assert_eq!(get_ini(two, "X").as_deref(), Some("1"));
    assert_eq!(get_ini(two, "Z"), None);
    assert_eq!(set_ini("A=1\n", "B", "2"), "A=1\nB=2\n");
    assert_eq!(set_ini("A=1", "B", "2"), "A=1\nB=2");
}

#[test]
fn a_child_check_s_output_reads_back_as_its_problems_or_no_profile() {
    let clean = "PC profile FOH Ableton PC: as it should be (220 entries)\n";
    assert_eq!(read_child_check(clean, ""), ChildCheck::Problems(vec![]));
    let two = "PROBLEM registry A\\B: 2 (0x2) (the profile wants 4 (0x4))\r\n\
               PROBLEM process x.exe affinity: 0xFFFF (the profile wants 0xF0)\r\n\
               PC profile FOH: 2 problems of 220 entries\r\n";
    assert_eq!(
        read_child_check(two, ""),
        ChildCheck::Problems(vec![
            "registry A\\B: 2 (0x2) (the profile wants 4 (0x4))".into(),
            "process x.exe affinity: 0xFFFF (the profile wants 0xF0)".into(),
        ])
    );
    // No summary line: the profile was not read (absent or broken).
    let absent = "dasio: the profile C:\\ProgramData\\dasio\\pc-profile.toml: not found\r\n";
    assert_eq!(
        read_child_check("", absent),
        ChildCheck::NoProfile(
            "dasio: the profile C:\\ProgramData\\dasio\\pc-profile.toml: not found".into()
        )
    );
    assert_eq!(
        read_child_check("", ""),
        ChildCheck::NoProfile(String::new())
    );
}

#[test]
fn a_kernel_setting_under_session_manager_needs_a_reboot() {
    // ReservedCpuSets is read at boot; apply said "nothing more" (FOH,
    // dasio#1, 5.11.2026).
    let a = Action::Reg {
        key: r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\kernel".into(),
        name: "ReservedCpuSets".into(),
        value: Value::Binary(vec![0xf0, 0xff, 0xff, 0xff, 0, 0, 0, 0]),
    };
    assert_eq!(takes_effect(&a), "a reboot");
}

#[test]
fn an_environment_value_under_session_manager_needs_a_new_logon() {
    // Session Manager\Environment is read at logon (and by services at
    // boot), not only at boot: "a reboot" overstates it (review of the
    // Session Manager arm, dasio#1, 5.11.2026).
    let a = Action::Reg {
        key: r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment".into(),
        name: "PATH".into(),
        value: Value::String(r"C:\Windows".into()),
    };
    assert_eq!(takes_effect(&a), "a new logon (a reboot for services)");
}

#[test]
fn a_drive_below_its_floor_is_a_problem_and_nothing_is_applied_for_it() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    // FOH's C: on 9.11.2026: 26.5 GB free (dasio#1).
    pc.free.insert("C:".into(), 53 << 29);
    let text: Vec<String> = check(&p, &pc).iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        vec![
            "disk C: free space: below the floor (the profile wants at least 50 GB) — a full C: stalls the DAWs"
                .to_string()
        ]
    );
    // Free space is only watched: no action can set it.
    assert_eq!(plan(&p, &pc), vec![]);
    // The service logs a problem again only when its text changes: while
    // the drive stays under its floor, the text stays the same.
    pc.free.insert("C:".into(), 30 << 30);
    let again: Vec<String> = check(&p, &pc).iter().map(ToString::to_string).collect();
    assert_eq!(again, text);
}

#[test]
fn free_space_reads_in_gb_rounded_down() {
    assert_eq!(gb_down(53 << 29), "26.5 GB");
    assert_eq!(gb_down((50 << 30) - 1), "49.9 GB");
    assert_eq!(gb_down(50 << 30), "50.0 GB");
    assert_eq!(gb_down(0), "0.0 GB");
    assert_eq!(gb_down(u64::MAX), "17179869183.9 GB");
}

#[test]
fn a_drive_exactly_at_its_floor_is_fine_and_one_byte_under_is_not() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    pc.free.insert("C:".into(), 50 << 30);
    assert_eq!(check(&p, &pc), vec![]);
    pc.free.insert("C:".into(), (50 << 30) - 1);
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].found, "below the floor");
    assert_eq!(problems[0].want, "at least 50 GB");
}

#[test]
fn an_unreadable_drive_is_a_problem_that_says_why() {
    let p = parse(PROFILE).unwrap();
    let mut pc = matching_pc();
    pc.unreadable.push("C:".into());
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].what, "disk C: free space");
    assert_eq!(problems[0].found, "unreadable (the device is not ready)");
}

#[test]
fn a_disk_entry_with_a_mistake_is_refused() {
    for drive in ["C", "CC", "CC:", "1:", "C:\\", ""] {
        let text = format!("name = \"x\"\n[[disk]]\ndrive = '{drive}'\nmin_free_gb = 10\n");
        assert!(
            parse(&text).unwrap_err().contains("a letter and a colon"),
            "drive {drive:?}"
        );
    }
    let zero = "name = \"x\"\n[[disk]]\ndrive = \"D:\"\nmin_free_gb = 0\n";
    assert!(parse(zero).unwrap_err().contains("above 0"));
    let typo = "name = \"x\"\n[[disk]]\ndrive = \"D:\"\nmin_free = 10\n";
    assert!(parse(typo).is_err(), "unknown fields are refused");
    let ok = "name = \"x\"\n[[disk]]\ndrive = \"d:\"\nmin_free_gb = 1\n";
    assert_eq!(parse(ok).unwrap().disk[0].min_free_gb, 1);
}

// A NIC's address settings are REG_MULTI_SZ (e.g. `SubnetMask`): the
// profile holds them as lists, compares them as lists and writes them back
// as REG_MULTI_SZ, never as binary (dasio#26).
const MULTI: &str = r#"
name = "FOH"

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{x}'
name = "SubnetMask"
multi_sz = ["255.255.255.0"]
why = "the Dante network is 10.77.7.0/24 (router)"
"#;

#[test]
fn a_list_value_is_checked_and_applied_as_a_list() {
    let p = parse(MULTI).unwrap();
    let key = r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{x}";
    let mut pc = Pc::default();
    pc.reg.insert(
        (key.into(), "SubnetMask".into()),
        Value::MultiSz(vec!["255.255.255.0".into()]),
    );
    assert_eq!(check(&p, &pc), vec![]);
    assert_eq!(plan(&p, &pc), vec![]);
    // The /23 the PC had before 9.11.
    pc.reg.insert(
        (key.into(), "SubnetMask".into()),
        Value::MultiSz(vec!["255.255.254.0".into()]),
    );
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].found, r#"["255.255.254.0"]"#);
    assert_eq!(problems[0].want, r#"["255.255.255.0"]"#);
    assert_eq!(
        plan(&p, &pc),
        vec![Action::Reg {
            key: key.into(),
            name: "SubnetMask".into(),
            value: Value::MultiSz(vec!["255.255.255.0".into()]),
        }]
    );
}

#[test]
fn a_list_value_with_another_kind_is_refused() {
    let both = "name = \"x\"\n[[registry]]\nkey = 'HKLM\\A'\nname = \"B\"\nmulti_sz = [\"1\"]\nstring = \"1\"\n";
    assert!(parse(both).unwrap_err().contains("multi_sz"));
}

#[test]
fn a_list_reads_and_writes_as_windows_stores_it() {
    let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    // One string: its NUL, then the list's NUL.
    assert_eq!(
        multi_sz_to_wide(&["255.255.255.0".into()]),
        w("255.255.255.0\0\0")
    );
    assert_eq!(multi_sz_to_wide(&["a".into(), "bc".into()]), w("a\0bc\0\0"));
    // An empty list is two NULs.
    assert_eq!(multi_sz_to_wide(&[]), w("\0\0"));
    // Read back.
    assert_eq!(multi_sz_from_wide(&w("a\0bc\0\0")), vec!["a", "bc"]);
    assert_eq!(multi_sz_from_wide(&w("\0\0")), Vec::<String>::new());
    assert_eq!(multi_sz_from_wide(&w("")), Vec::<String>::new());
    // A list without its terminators still reads; an empty string ends it.
    assert_eq!(multi_sz_from_wide(&w("a\0bc")), vec!["a", "bc"]);
    assert_eq!(multi_sz_from_wide(&w("a\0\0zz\0\0")), vec!["a"]);
}

#[test]
fn an_absent_value_may_be_fine_but_a_wrong_one_never_is() {
    // FOH, 10.11.2026: GigabyteUpdateService was uninstalled, so its
    // Start value is gone. The entry still catches a re-install (Start 2).
    let p = parse(
        r#"
name = "FOH"

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\GigabyteUpdateService'
name = "Start"
dword = 4
absent_ok = true
why = "off, or gone"

[[registry]]
key = 'HKLM\SYSTEM\CurrentControlSet\Services\MMCSS'
name = "Start"
dword = 4
"#,
    )
    .unwrap();
    let gigabyte = (
        r"HKLM\SYSTEM\CurrentControlSet\Services\GigabyteUpdateService".to_string(),
        "Start".to_string(),
    );
    let mmcss = (
        r"HKLM\SYSTEM\CurrentControlSet\Services\MMCSS".to_string(),
        "Start".to_string(),
    );
    // Both absent: only the entry without absent_ok is a problem, and only
    // its value is planned (nothing is created for the gone service).
    let mut pc = Pc::default();
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].what, format!(r"registry {}\Start", mmcss.0));
    assert_eq!(problems[0].found, "absent");
    assert_eq!(
        plan(&p, &pc),
        vec![Action::Reg {
            key: mmcss.0.clone(),
            name: "Start".into(),
            value: Value::Dword(4),
        }]
    );
    // The firmware re-installed the service: Start 2 is a problem again.
    pc.reg.insert(mmcss, Value::Dword(4));
    pc.reg.insert(gigabyte.clone(), Value::Dword(2));
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].found, "2 (0x2)");
    assert_eq!(problems[0].why, "off, or gone");
    assert_eq!(
        plan(&p, &pc),
        vec![Action::Reg {
            key: gigabyte.0.clone(),
            name: "Start".into(),
            value: Value::Dword(4),
        }]
    );
    // Its wanted value is fine too.
    pc.reg.insert(gigabyte, Value::Dword(4));
    assert_eq!(check(&p, &pc), vec![]);
    assert_eq!(plan(&p, &pc), vec![]);
    // An unreadable value is still a problem with absent_ok.
    pc.unreadable
        .push(r"HKLM\SYSTEM\CurrentControlSet\Services\GigabyteUpdateService".into());
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].found.starts_with("unreadable"), "{problems:?}");
}

/// `bcdedit /enum {current}` as FOH prints it (shortened; a list value
/// continues on indented lines).
const BCDEDIT: &str = "
Windows Boot Loader
-------------------
identifier              {current}
device                  partition=C:
path                    \\WINDOWS\\system32\\winload.efi
description             Windows 10
recoverysequence        {8f6c1a2e-0000-0000-0000-000000000000}
displaymessageoverride  Recovery
testsigning             Yes
nx                      OptIn
badmemorylist           0x1
                        0x2
disabledynamictick      Yes
";

#[test]
fn bcdedit_s_entry_reads_as_names_and_values() {
    let list = parse_bcdedit(BCDEDIT);
    assert_eq!(list[0], ("identifier".to_string(), "{current}".to_string()));
    let get = |n: &str| list.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    assert_eq!(
        get("description"),
        Some("Windows 10"),
        "a value with a space stays whole"
    );
    assert_eq!(get("testsigning"), Some("Yes"));
    assert_eq!(
        get("badmemorylist"),
        Some("0x1"),
        "a list keeps its first line"
    );
    assert_eq!(
        get("disabledynamictick"),
        Some("Yes"),
        "after a continued list"
    );
    assert!(get("Windows").is_none(), "the title is not a setting");
    assert!(list.iter().all(|(k, _)| !k.is_empty()));
    assert!(parse_bcdedit("no dashes here\nfoo bar\n").is_empty());
}

fn bcd_profile(entries: &str) -> Profile {
    parse(&format!("name = \"x\"\n{entries}")).unwrap()
}

#[test]
fn a_bcd_setting_as_wanted_is_no_problem_whatever_its_case() {
    let p = bcd_profile(
        "[[bcd]]\nname = \"disabledynamictick\"\nvalue = \"yes\"\n[[bcd]]\nname = \"TestSigning\"\nvalue = \"Yes\"\n",
    );
    let pc = Pc {
        bcd: parse_bcdedit(BCDEDIT),
        ..Pc::default()
    };
    assert!(check(&p, &pc).is_empty());
    assert_eq!(pc.bcd_reads.get(), 1, "bcdedit runs once per check");
}

#[test]
fn a_bcd_setting_off_or_absent_or_unreadable_is_a_problem() {
    let p = bcd_profile(
        "[[bcd]]\nname = \"disabledynamictick\"\nvalue = \"Yes\"\nwhy = \"no tick on idle CPUs\"\n[[bcd]]\nname = \"useplatformclock\"\nvalue = \"No\"\n",
    );
    let pc = Pc {
        bcd: vec![("disabledynamictick".into(), "No".into())],
        ..Pc::default()
    };
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 2);
    assert_eq!(
        problems[0].to_string(),
        "bcd disabledynamictick: \"No\" (the profile wants \"Yes\") — no tick on idle CPUs"
    );
    assert_eq!(problems[1].what, "bcd useplatformclock");
    assert_eq!(problems[1].found, "absent");
    let down = Pc {
        bcd_err: Some("bcdedit: exit code: 1 access denied".into()),
        ..Pc::default()
    };
    let problems = check(&p, &down);
    assert_eq!(problems.len(), 2);
    assert!(
        problems
            .iter()
            .all(|x| x.found == "unreadable (bcdedit: exit code: 1 access denied)")
    );
}

#[test]
fn no_bcd_entries_never_run_bcdedit() {
    let pc = Pc::default();
    let _ = check(&bcd_profile(""), &pc);
    assert_eq!(pc.bcd_reads.get(), 0);
}

#[test]
fn a_bcd_entry_needs_one_word_and_a_value() {
    assert!(
        parse("name = \"x\"\n[[bcd]]\nname = \"disable dynamictick\"\nvalue = \"Yes\"\n").is_err()
    );
    assert!(parse("name = \"x\"\n[[bcd]]\nname = \"\"\nvalue = \"Yes\"\n").is_err());
    assert!(parse("name = \"x\"\n[[bcd]]\nname = \"nx\"\nvalue = \" \"\n").is_err());
    assert!(
        parse("name = \"x\"\n[[bcd]]\nname = \"nx\"\nvalue = \"OptIn\"\nwant = 1\n").is_err(),
        "unknown keys are refused"
    );
    assert_eq!(
        bcd_profile("[[bcd]]\nname = \"nx\"\nvalue = \"OptIn\"\n").bcd[0].value,
        "OptIn"
    );
}

#[test]
fn every_kind_of_entry_counts() {
    // Every kind present, each a different number of times: a sum with any
    // term dropped or another operator gives another total.
    let mut text = String::from("name = \"x\"\n");
    text += "[[registry]]\nkey = 'HKLM\\\\A'\nname = \"B\"\ndword = 1\n";
    for k in ["a", "b"] {
        text += &format!("[[ini]]\nfile = 'C:\\\\l.ini'\nkey = \"{k}\"\nvalue = \"1\"\n");
    }
    for e in ["a.exe", "b.exe", "c.exe"] {
        text += &format!("[[process]]\nexe = \"{e}\"\npriority = \"high\"\n");
    }
    for d in ["C:", "D:", "E:", "F:"] {
        text += &format!("[[disk]]\ndrive = \"{d}\"\nmin_free_gb = 1\n");
    }
    for n in [
        "nx",
        "testsigning",
        "disabledynamictick",
        "useplatformclock",
        "tscsyncpolicy",
    ] {
        text += &format!("[[bcd]]\nname = \"{n}\"\nvalue = \"Yes\"\n");
    }
    for n in ["a", "b", "c", "d", "e", "f"] {
        text += &format!("[[task]]\nname = '\\{n}'\nenabled = true\n");
    }
    let p = parse(&text).unwrap();
    assert_eq!(
        (
            p.registry.len(),
            p.ini.len(),
            p.process.len(),
            p.disk.len(),
            p.bcd.len(),
            p.task.len()
        ),
        (1, 2, 3, 4, 5, 6)
    );
    assert_eq!(p.entries(), 21, "every kind counts, [[task]] too");
}

/// FOH's fleetbackup task as `schtasks /query /tn \fleetbackup-main /xml`
/// printed it on 11.11.2026 (blank lines between elements, as schtasks
/// writes them).
const FLEETBACKUP_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\n<Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\n  <RegistrationInfo>\n\n    <Description>fleetbackup agent, daily backup set main (VSS)</Description>\n\n    <URI>\\fleetbackup-main</URI>\n\n  </RegistrationInfo>\n\n  <Settings>\n\n    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\n    <RestartOnFailure>\n\n      <Count>3</Count>\n\n      <Interval>PT15M</Interval>\n\n    </RestartOnFailure>\n\n    <IdleSettings>\n\n      <StopOnIdleEnd>false</StopOnIdleEnd>\n\n    </IdleSettings>\n\n    <UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>\n\n  </Settings>\n\n  <Triggers>\n\n    <CalendarTrigger>\n\n      <StartBoundary>2026-11-05T04:00:00+01:00</StartBoundary>\n\n      <ScheduleByDay>\n\n        <DaysInterval>1</DaysInterval>\n\n      </ScheduleByDay>\n\n    </CalendarTrigger>\n\n  </Triggers>\n\n  <Actions Context=\"Author\">\n\n    <Exec>\n\n      <Command>\"C:\\Program Files\\fleetbackup\\fbagent.exe\"</Command>\n\n    </Exec>\n\n  </Actions>\n\n</Task>\n";

#[test]
fn a_task_s_xml_reads_as_on_and_its_triggers() {
    assert_eq!(
        parse_task_xml(FLEETBACKUP_XML),
        TaskState {
            enabled: true,
            triggers: vec!["daily 04:00".into()],
        }
    );
    // The 5.11 shape before the fix: every 2 h from midnight, disabled.
    let old = "<Task><Settings><Enabled>false</Enabled><IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd></IdleSettings></Settings><Triggers>\
        <CalendarTrigger><Repetition><Interval>PT2H</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition>\
        <StartBoundary>2026-10-01T00:00:00</StartBoundary><ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger>\
        </Triggers></Task>";
    assert_eq!(
        parse_task_xml(old),
        TaskState {
            enabled: false,
            triggers: vec!["daily 00:00 every PT2H".into()],
        }
    );
    let many = "<Task><Settings><Enabled>true</Enabled></Settings><Triggers>\
        <BootTrigger><Enabled>false</Enabled></BootTrigger>\
        <LogonTrigger id=\"t2\"><UserId>X</UserId></LogonTrigger>\
        <TimeTrigger><StartBoundary>2026-11-11T13:05:00</StartBoundary></TimeTrigger>\
        <CalendarTrigger><StartBoundary>2026-11-11T02:30:00</StartBoundary><ScheduleByDay><DaysInterval>3</DaysInterval></ScheduleByDay></CalendarTrigger>\
        <CalendarTrigger><StartBoundary>2026-11-11T06:00:00</StartBoundary><ScheduleByWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek></CalendarTrigger>\
        <CalendarTrigger><StartBoundary>2026-11-11T07:00:00</StartBoundary><ScheduleByMonth><Months/></ScheduleByMonth></CalendarTrigger>\
        <IdleTrigger/><EventTrigger><Subscription>x</Subscription></EventTrigger><SessionStateChangeTrigger><StateChange>x</StateChange></SessionStateChangeTrigger>\
        </Triggers></Task>";
    assert_eq!(
        parse_task_xml(many).triggers,
        [
            "boot (off)",
            "logon",
            "time 13:05",
            "every 3d 02:30",
            "weekly 06:00",
            "monthly 07:00",
            "idle",
            "event",
            "other:SessionStateChangeTrigger",
        ]
    );
    assert!(parse_task_xml(many).enabled);
    assert!(parse_task_xml("<Task></Task>").triggers.is_empty());
}

fn task_profile(entries: &str) -> Profile {
    parse(&format!("name = \"x\"\n{entries}")).unwrap()
}

#[test]
fn a_task_as_wanted_is_no_problem() {
    let p = task_profile(
        "[[task]]\nname = '\\fleetbackup-main'\nenabled = true\ntriggers = [\"daily 04:00\"]\n",
    );
    let pc = Pc {
        tasks: HashMap::from([("\\fleetbackup-main".into(), parse_task_xml(FLEETBACKUP_XML))]),
        ..Pc::default()
    };
    assert!(check(&p, &pc).is_empty());
}

#[test]
fn a_task_off_retriggered_absent_or_unreadable_is_a_problem() {
    let p = task_profile(
        "[[task]]\nname = '\\fb'\nenabled = true\ntriggers = [\"daily 04:00\"]\nwhy = \"its VSS stalled band\"\n\
         [[task]]\nname = '\\gone'\nenabled = false\n\
         [[task]]\nname = '\\maybe'\nenabled = false\nabsent_ok = true\n\
         [[task]]\nname = '\\locked'\ntriggers = []\n",
    );
    let pc = Pc {
        tasks: HashMap::from([(
            "\\fb".into(),
            TaskState {
                enabled: false,
                triggers: vec!["daily 00:00 every PT2H".into()],
            },
        )]),
        task_err: vec!["\\locked".into()],
        ..Pc::default()
    };
    let problems: Vec<String> = check(&p, &pc).iter().map(|x| x.to_string()).collect();
    assert_eq!(
        problems,
        [
            "task \\fb: off (the profile wants on) — its VSS stalled band",
            "task \\fb triggers: [\"daily 00:00 every PT2H\"] (the profile wants [\"daily 04:00\"]) — its VSS stalled band",
            "task \\gone: absent (the profile wants the task)",
            "task \\locked: unreadable (access denied) (the profile wants the task)",
        ]
    );
    // Only what an entry names is compared.
    let only_on = task_profile("[[task]]\nname = '\\fb'\nenabled = false\n");
    assert!(check(&only_on, &pc).is_empty());
}

#[test]
fn a_task_entry_needs_a_path_and_something_to_check() {
    assert!(parse("name = \"x\"\n[[task]]\nname = 'fb'\nenabled = true\n").is_err());
    assert!(parse("name = \"x\"\n[[task]]\nname = '\\fb'\n").is_err());
    assert!(parse("name = \"x\"\n[[task]]\nname = '\\fb'\nenabled = true\nrun = 1\n").is_err());
    assert!(parse("name = \"x\"\n[[task]]\nname = '\\fb'\ntriggers = []\n").is_ok());
}
