use std::collections::HashMap;

use super::*;

/// A PC as a test sets it up: the reader the check sees.
#[derive(Default)]
struct Pc {
    reg: HashMap<(String, String), Value>,
    unreadable: Vec<String>,
    ini: HashMap<(String, String), String>,
    procs: HashMap<String, ProcState>,
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
