//! The services the service holds off itself (dasio#158).

use super::super::hold::{
    ACCESS_DENIED, DISABLED, Fix, HELD, Held, decide, describe, not_held, registry_fallback,
    set_start_with, start_name,
};
use super::super::*;

const USO: Held = Held {
    service: "UsoSvc",
    start: 4,
};

/// Delivery Optimization: Windows' servicing set its `Start` 4 -> 3 on FOH
/// right after an SPP start (18.11.2026 05:03 CET, no SCM event: a direct
/// registry write), and the check only reported it (dasio#26).
const DO_SVC: Held = Held {
    service: "DoSvc",
    start: 4,
};

#[test]
fn uso_svc_and_do_svc_are_held_disabled() {
    assert_eq!(HELD, &[USO, DO_SVC]);
    assert_eq!(DISABLED, 4);
}

#[test]
fn a_do_svc_reset_to_manual_is_set_back_and_stopped() {
    assert_eq!(
        decide(&DO_SVC, Some(3), true),
        Some(Fix {
            was: Some(3),
            stop: true
        })
    );
    assert_eq!(
        describe(
            &DO_SVC,
            &Fix {
                was: Some(3),
                stop: false
            },
            &Ok(()),
            &Ok(())
        ),
        "DoSvc held off (dasio#158): start was 3 (manual), set to 4 (disabled)"
    );
}

/// The hold and the baseline's check agree: the baseline holds each held
/// service's `Start` at the same value, so the check after a hold is clean.
#[test]
fn the_baseline_checks_each_held_service_at_the_same_start() {
    let b = baseline().unwrap();
    for h in HELD {
        let key = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{}", h.service);
        let e = b
            .registry
            .iter()
            .find(|e| e.key == key && e.name == "Start")
            .unwrap_or_else(|| panic!("the baseline checks {key}"));
        assert_eq!(e.dword, Some(h.start), "{key}");
    }
}

#[test]
fn not_installed_is_left_alone() {
    assert_eq!(decide(&USO, None, false), None);
    assert_eq!(decide(&USO, None, true), None);
}

#[test]
fn disabled_and_stopped_is_left_alone() {
    assert_eq!(decide(&USO, Some(4), false), None);
}

#[test]
fn a_reset_start_is_set_back_and_a_running_one_stopped() {
    assert_eq!(
        decide(&USO, Some(2), true),
        Some(Fix {
            was: Some(2),
            stop: true
        })
    );
    assert_eq!(
        decide(&USO, Some(2), false),
        Some(Fix {
            was: Some(2),
            stop: false
        })
    );
    assert_eq!(
        decide(&USO, Some(4), true),
        Some(Fix {
            was: None,
            stop: true
        })
    );
}

/// Only a service held disabled is stopped: one held at manual may run.
#[test]
fn a_service_held_at_manual_is_not_stopped() {
    let manual = Held {
        service: "X",
        start: 3,
    };
    assert_eq!(decide(&manual, Some(3), true), None);
    assert_eq!(
        decide(&manual, Some(2), true),
        Some(Fix {
            was: Some(2),
            stop: false
        })
    );
}

#[test]
fn start_types_by_name() {
    let names: Vec<_> = (0..=5).map(start_name).collect();
    assert_eq!(
        names,
        [
            "boot",
            "system",
            "automatic",
            "manual",
            "disabled",
            "unknown"
        ]
    );
}

#[test]
fn the_log_line_names_what_was_done() {
    let both = Fix {
        was: Some(2),
        stop: true,
    };
    assert_eq!(
        describe(&USO, &both, &Ok(()), &Ok(())),
        "UsoSvc held off (dasio#158): \
         start was 2 (automatic), set to 4 (disabled); it was running: stopped"
    );
    assert_eq!(
        describe(
            &USO,
            &both,
            &Err("Access is denied. (os error 5)".into()),
            &Err("still stopping after 30 s".into())
        ),
        "UsoSvc held off (dasio#158): \
         start is 2 (automatic), not set to 4 (disabled): Access is denied. (os error 5); \
         it runs and did not stop: still stopping after 30 s"
    );
    let start_only = Fix {
        was: Some(3),
        stop: false,
    };
    // A stop result is not reported when no stop was asked.
    assert_eq!(
        describe(&USO, &start_only, &Ok(()), &Err("x".into())),
        "UsoSvc held off (dasio#158): start was 3 (manual), set to 4 (disabled)"
    );
    let stop_only = Fix {
        was: None,
        stop: true,
    };
    // A set result is not reported when no set was asked.
    assert_eq!(
        describe(&USO, &stop_only, &Err("x".into()), &Ok(())),
        "UsoSvc held off (dasio#158): it was running: stopped"
    );
}

#[test]
fn a_service_it_cannot_look_at_is_named() {
    assert_eq!(
        not_held(&USO, "its Start is unreadable: os error 5"),
        "UsoSvc not held (dasio#158): its Start is unreadable: os error 5"
    );
}

/// The service's check thread holds them before every check, in process
/// (both kinds, start and every 10 min), off the media CPUs, at the lowest
/// priority.
#[test]
fn the_check_thread_holds_before_it_checks() {
    let watch = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run/profile.rs"),
    )
    .unwrap()
    .replace("\r\n", "\n");
    let hold = watch
        .find("crate::pc_profile::hold::windows::hold_services()")
        .expect("the check thread holds");
    let check = watch.find("let found = check(own);").expect("the check");
    assert!(hold < check, "held before the check");
    let low = watch
        .find("lowest_priority();")
        .expect("the thread's priority");
    assert!(low < hold, "lowered before the hold");
    let off = watch
        .find("off_media_cpus(own);")
        .expect("the thread's CPUs");
    assert!(off < hold, "off the media CPUs before the hold");
    assert_eq!(watch.matches("hold_services()").count(), 1);
}

/// DoSvc is a protected service (`sc qprotection DoSvc`: WINDOWS LIGHT): the
/// SCM refuses its start type even to SYSTEM ("Access is denied. (os error
/// 5)", dsg 18.11.2026 07:10:43Z), while SYSTEM may write its registry key.
/// Only that refusal goes to the registry; any other SCM error stays one.
#[test]
fn only_an_access_denied_start_goes_to_the_registry() {
    assert_eq!(ACCESS_DENIED, 5);
    assert!(registry_fallback(Some(5)));
    assert!(!registry_fallback(Some(1060)));
    assert!(!registry_fallback(Some(1072)));
    assert!(!registry_fallback(None));
}

/// The SCM's answer decides whether the registry is written at all, and
/// the write's own result is what is reported.
#[test]
fn the_registry_is_written_only_after_an_access_denied() {
    let denied = || std::io::Error::from_raw_os_error(5);
    let mut wrote = 0;
    assert_eq!(
        set_start_with(Ok(()), || {
            wrote += 1;
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(wrote, 0, "an SCM success writes nothing");
    assert_eq!(
        set_start_with(Err(denied()), || {
            wrote += 1;
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(wrote, 1, "access denied writes the registry once");
    let failed = set_start_with(Err(denied()), || Err("os error 2".into())).unwrap_err();
    assert!(failed.contains("the SCM refused it"), "{failed}");
    assert!(
        failed.ends_with("the registry write failed: os error 2"),
        "{failed}"
    );
    let other = set_start_with(Err(std::io::Error::from_raw_os_error(1060)), || {
        wrote += 1;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(wrote, 1, "another SCM error writes nothing");
    assert_eq!(other, std::io::Error::from_raw_os_error(1060).to_string());
}
