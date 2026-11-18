//! Registry entries in SYSTEM's own hive (dasio#191): dasio runs as
//! SYSTEM, so an ASIO driver it loads keeps its per-user settings there
//! (dsg's SoundGrid ASIO buffer, `Buffer Size(Samples)`).

use super::*;

const SG: &str = r"HKU\S-1-5-18\Software\Waves\SoundGrid\ASIO Driver\Configuration";

fn entry(key: &str) -> String {
    format!(
        "name = \"x\"\n[[registry]]\nkey = '{key}'\nname = \"Buffer Size(Samples)\"\nbinary = \"80000000\"\n"
    )
}

#[test]
fn a_key_names_its_root() {
    assert_eq!(
        split_root(r"HKLM\SYSTEM\X"),
        Ok((Root::Machine, r"SYSTEM\X"))
    );
    assert_eq!(
        split_root(SG),
        Ok((
            Root::Users,
            r"S-1-5-18\Software\Waves\SoundGrid\ASIO Driver\Configuration"
        ))
    );
}

#[test]
fn another_user_s_hive_or_another_root_is_refused_naming_why() {
    assert_eq!(
        split_root(r"HKU\S-1-5-21-1-2-3-1001\Software\X"),
        Err(r"HKU\S-1-5-21-1-2-3-1001\Software\X: under HKU\ only SYSTEM's own hive (HKU\S-1-5-18\): another user's hive is loaded only while that user is logged on".into())
    );
    // .DEFAULT is SYSTEM's hive under another name: one spelling only.
    assert!(split_root(r"HKU\.DEFAULT\Software\X").is_err());
    // A hive whose name only starts like SYSTEM's.
    assert!(split_root(r"HKU\S-1-5-18_Classes\X").is_err());
    assert_eq!(
        split_root(r"HKCU\Software\X"),
        Err(r"HKCU\Software\X: only HKLM\ or HKU\S-1-5-18\ keys".into())
    );
}

#[test]
fn a_profile_takes_system_s_hive_and_refuses_another_user_s() {
    let p = parse(&entry(SG)).unwrap();
    assert_eq!(p.registry[0].key, SG);
    let err = parse(&entry(r"HKU\S-1-5-21-1-2-3-1001\Software\X")).unwrap_err();
    assert!(
        err.starts_with(r"registry HKU\S-1-5-21-1-2-3-1001\Software\X: under HKU\ only"),
        "{err}"
    );
    // Any other root stays a problem of its entry, as before: a profile
    // that loaded before still loads.
    assert!(parse(&entry(r"HKCU\Software\X")).is_ok());
}

#[test]
fn a_value_in_system_s_hive_is_checked_and_planned_like_any_other() {
    let p = parse(&entry(SG)).unwrap();
    let mut pc = Pc::default();
    pc.reg.insert(
        (SG.into(), "Buffer Size(Samples)".into()),
        Value::Binary(vec![0, 1, 0, 0]),
    );
    let problems = check(&p, &pc);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].found, "00010000");
    assert_eq!(problems[0].want, "80000000");
    assert_eq!(
        plan(&p, &pc),
        vec![Action::Reg {
            key: SG.into(),
            name: "Buffer Size(Samples)".into(),
            value: Value::Binary(vec![0x80, 0, 0, 0]),
        }]
    );
    pc.reg.insert(
        (SG.into(), "Buffer Size(Samples)".into()),
        Value::Binary(vec![0x80, 0, 0, 0]),
    );
    assert!(check(&p, &pc).is_empty());
}
