//! Power-plan values go through powercfg (dasio#128): a value the active
//! scheme never stored stays absent after a plain registry write (mbc,
//! 16.11.2026: processor max, ASPM and hibernate).

use super::super::*;

const SCHEME: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\{active_scheme}";
const SUB: &str = "54533251-82be-4824-96c1-47b60b740d00";
const SET: &str = "bc5038f7-23e0-4960-96da-33abaf5935ec";

fn reg(key: &str, name: &str, value: Value) -> Action {
    Action::Reg {
        key: key.into(),
        name: name.into(),
        value,
    }
}

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn an_ac_value_of_the_active_plan_parses_to_its_subgroup_and_setting() {
    let key = format!(r"{SCHEME}\{SUB}\{SET}");
    assert_eq!(
        power_setting(&key, "ACSettingIndex"),
        Some(PowerSetting {
            subgroup: SUB.into(),
            setting: SET.into(),
            ac: true,
        })
    );
    assert_eq!(
        power_setting(&key, "DCSettingIndex").map(|p| p.ac),
        Some(false)
    );
}

#[test]
fn anything_else_is_no_power_setting() {
    let key = format!(r"{SCHEME}\{SUB}\{SET}");
    // Another value name of the same key.
    assert_eq!(power_setting(&key, "Attributes"), None);
    // The subgroup alone, or one level deeper.
    assert_eq!(
        power_setting(&format!(r"{SCHEME}\{SUB}"), "ACSettingIndex"),
        None
    );
    assert_eq!(
        power_setting(&format!(r"{SCHEME}\{SUB}\{SET}\x"), "ACSettingIndex"),
        None
    );
    // A part that is not a GUID.
    assert_eq!(
        power_setting(&format!(r"{SCHEME}\{SUB}\not-a-guid"), "ACSettingIndex"),
        None
    );
    // GUID-shaped but not one: a part of the right length with a non-hex
    // digit, and all hex digits with a part one short.
    assert_eq!(
        power_setting(
            &format!(r"{SCHEME}\{SUB}\bc5038f7-23e0-4960-96da-33abaf5935eg"),
            "ACSettingIndex"
        ),
        None
    );
    assert_eq!(
        power_setting(
            &format!(r"{SCHEME}\{SUB}\bc5038f7-23e0-4960-96da-33abaf5935e"),
            "ACSettingIndex"
        ),
        None
    );
    // A literal scheme GUID is a plain registry entry, not the active plan.
    assert_eq!(
        power_setting(
            &format!(
                r"HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c\{SUB}\{SET}"
            ),
            "ACSettingIndex"
        ),
        None
    );
}

#[test]
fn apply_sets_a_power_value_with_powercfg_and_activates_the_plan() {
    let a = reg(
        &format!(r"{SCHEME}\{SUB}\{SET}"),
        "ACSettingIndex",
        Value::Dword(100),
    );
    assert_eq!(
        powercfg_commands(&a),
        Some(vec![
            argv(&["/setacvalueindex", "scheme_current", SUB, SET, "100"]),
            argv(&["/setactive", "scheme_current"]),
        ])
    );
    let dc = reg(
        &format!(r"{SCHEME}\{SUB}\{SET}"),
        "DCSettingIndex",
        Value::Dword(0),
    );
    assert_eq!(
        powercfg_commands(&dc).unwrap()[0],
        argv(&["/setdcvalueindex", "scheme_current", SUB, SET, "0"])
    );
}

#[test]
fn every_other_action_stays_a_registry_or_ini_write() {
    let other = reg(r"HKLM\SYSTEM\X", "ACSettingIndex", Value::Dword(1));
    assert_eq!(powercfg_commands(&other), None);
    // A power value that is not a DWORD: powercfg takes only an index.
    let s = reg(
        &format!(r"{SCHEME}\{SUB}\{SET}"),
        "ACSettingIndex",
        Value::String("1".into()),
    );
    assert_eq!(powercfg_commands(&s), None);
    let ini = Action::Ini {
        file: "a.ini".into(),
        key: "K".into(),
        value: "1".into(),
    };
    assert_eq!(powercfg_commands(&ini), None);
}

#[test]
fn apply_names_the_step_it_takes() {
    let a = reg(
        &format!(r"{SCHEME}\{SUB}\{SET}"),
        "ACSettingIndex",
        Value::Dword(100),
    );
    let d = describe(&a);
    assert!(
        d.contains(&format!(
            "powercfg /setacvalueindex scheme_current {SUB} {SET} 100"
        )),
        "{d}"
    );
    assert!(d.contains("powercfg /setactive scheme_current"), "{d}");
    assert!(takes_effect(&a).contains("powercfg /setactive"));
    assert!(!takes_effect(&a).contains("install.ps1"));
    let r = reg(r"HKLM\SYSTEM\X", "Y", Value::Dword(1));
    assert_eq!(describe(&r), r"registry HKLM\SYSTEM\X\Y = Dword(1)");
    let ini = Action::Ini {
        file: "a.ini".into(),
        key: "K".into(),
        value: "1".into(),
    };
    assert_eq!(describe(&ini), "a.ini K=1");
}

#[test]
fn every_power_value_of_the_baseline_goes_through_powercfg() {
    // A new baseline power entry cannot fall back to a raw registry write.
    let b = baseline().unwrap();
    let power: Vec<_> = b
        .registry
        .iter()
        .filter(|e| e.key.contains("{active_scheme}"))
        .collect();
    assert!(power.len() >= 7, "{}", power.len());
    for e in power {
        let a = Action::Reg {
            key: e.key.clone(),
            name: e.name.clone(),
            value: want_reg(e).unwrap(),
        };
        assert!(powercfg_commands(&a).is_some(), "{} {}", e.key, e.name);
    }
}
