//! The baseline holds every proven setting of the FOH and mbc profiles
//! (dasio#128, owner 16.11.2026: the baseline had 72 entries while FOH's
//! own profile had 235, and dsg, set up from the baseline, ran Defender,
//! Edge's updater and 216 tasks). `fold-keys.txt` lists every non-site key
//! of both profiles with generic names; the private copies stay on the dev
//! box.

use super::super::*;

const KEYS: &str = include_str!("fold-keys.txt");

/// The fixture's lines, without comments and blank lines.
fn lines() -> Vec<&'static str> {
    KEYS.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

#[test]
fn the_baseline_holds_every_non_site_key_of_the_foh_and_mbc_profiles() {
    let b = baseline().unwrap();
    let mut missing = Vec::new();
    for line in lines() {
        let parts: Vec<&str> = line.split('|').collect();
        let held = match parts.as_slice() {
            ["registry", key, name] => b
                .registry
                .iter()
                .any(|e| e.key.eq_ignore_ascii_case(key) && e.name.eq_ignore_ascii_case(name)),
            ["ini", file, key] => b
                .ini
                .iter()
                .any(|e| e.file.eq_ignore_ascii_case(file) && e.key == *key),
            ["bcd", name] => b.bcd.iter().any(|e| e.name.eq_ignore_ascii_case(name)),
            ["process", exe] => b.process.iter().any(|e| e.exe.eq_ignore_ascii_case(exe)),
            // FOH's DriverVersion pin moved to the hardware-ID driver entry
            // (dasio#128, 17.11.2026).
            ["nic_driver", id] => b
                .nic_driver
                .iter()
                .any(|e| e.hardware_ids.iter().any(|p| nic_driver::id_matches(p, id))),
            _ => panic!("fold-keys.txt: {line:?} is not kind|key|name, kind|file|key or kind|name"),
        };
        if !held {
            missing.push(line);
        }
    }
    assert!(
        missing.is_empty(),
        "the baseline misses {} proven FOH/mbc key(s): {missing:#?}",
        missing.len()
    );
}

#[test]
fn the_fold_fixture_is_whole() {
    // An emptied or cut fixture would pass the test above for nothing:
    // 207 keys on 16.11.2026: 54 registry, 150 ini, 2 bcd, 1 process; on
    // 17.11.2026 the registry DriverVersion pin became the nic_driver line
    // (the same key, now by hardware ID: 53 registry + 1 nic_driver).
    let l = lines();
    assert!(l.len() >= 207, "{} keys", l.len());
    let count = |kind: &str| l.iter().filter(|x| x.starts_with(kind)).count();
    assert!(count("registry|") >= 53, "{}", count("registry|"));
    assert_eq!(count("nic_driver|"), 1);
    assert!(count("ini|") >= 150, "{}", count("ini|"));
    assert_eq!(count("bcd|"), 2);
    assert_eq!(count("process|"), 1);
}

#[test]
fn the_fold_fixture_names_no_site_data() {
    // Generic names only (no site data in the repo): no GUID of one PC, no
    // device path, no address, no exe but dantesync's.
    for line in lines() {
        let low = line.to_ascii_lowercase();
        assert!(!low.contains(r"\enum\"), "a device path: {line}");
        assert!(!low.contains("tcpip"), "an adapter's address: {line}");
        assert!(!low.contains("dasio-"), "a dasio instance: {line}");
        let guid = line.split('{').skip(1).any(|rest| {
            rest.split('}')
                .next()
                .is_some_and(|g| g.len() == 36 && g.matches('-').count() == 4)
        });
        assert!(!guid, "a GUID: {line}");
        let dotted = line
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .any(|w| w.split('.').filter(|p| !p.is_empty()).count() == 4);
        assert!(!dotted, "an address: {line}");
        for word in low.split(['|', '\\', ' ', ',']) {
            if word.ends_with(".exe") {
                assert_eq!(word, "dsync.exe", "a site exe: {line}");
            }
        }
    }
}
