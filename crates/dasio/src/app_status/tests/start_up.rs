//! The start-up window in the status (dasio#28, owner 23.11.2026): the
//! counts shown are the start-up's while it is on, the running ones after
//! it, with the start-up window's own beside them. Its keys come last in
//! the JSON object, so no older key run is split.

use super::*;

fn in_window<const N: usize>() -> Part<N> {
    Part {
        in_window: true,
        startup: [0; N],
    }
}

#[test]
fn a_daw_instance_in_its_start_up_window_says_so() {
    let f = Facts {
        startup: in_window(),
        ..master()
    };
    assert_eq!(
        line(&f),
        "DAW attached · 64 @ 96 kHz · in 1.67 / out 5.67 ms · late 0/min (95 total) · start-up (first 5 min) · kernel TX"
    );
    let j = json(&f);
    assert!(
        j.ends_with("\"ktx_failed\":0,\"phase\":\"start-up\"}"),
        "{j}"
    );
    assert!(
        !j.contains("startup_"),
        "no start-up keys in the window: {j}"
    );
}

#[test]
fn a_daw_instance_after_its_window_shows_the_start_up_counts_beside_the_running_ones() {
    let f = Facts {
        startup: Part {
            in_window: false,
            startup: [12, 0, 2, 0],
        },
        ..master()
    };
    assert_eq!(
        line(&f),
        "DAW attached · 64 @ 96 kHz · in 1.67 / out 5.67 ms · late 0/min (95 total) · at start-up: late 12, stale 2 · kernel TX"
    );
    let j = json(&f);
    assert!(
        j.ends_with(concat!(
            "\"ktx_failed\":0,\"phase\":\"running\",\"startup_late_blocks\":12,",
            "\"startup_driver_missed\":0,\"startup_stale\":2,\"startup_waited\":0}"
        )),
        "{j}"
    );
    assert!(dstack::ctl::app_object(&j).is_some(), "{j}");
    let all = Facts {
        startup: Part {
            in_window: false,
            startup: [1, 2, 3, 4],
        },
        ..master()
    };
    assert!(
        line(&all).contains(" · at start-up: late 1, driver missed 2, stale 3, waited 4 · "),
        "{}",
        line(&all)
    );
}

#[test]
fn a_start_up_window_with_nothing_counted_leaves_the_line_as_it_was() {
    // master() has the default part: after the window, all 0.
    assert!(!line(&master()).contains("start-up"), "{}", line(&master()));
    assert!(
        json(&master()).ends_with(concat!(
            ",\"phase\":\"running\",\"startup_late_blocks\":0,\"startup_driver_missed\":0,",
            "\"startup_stale\":0,\"startup_waited\":0}"
        )),
        "{}",
        json(&master())
    );
}

#[test]
fn a_bridge_in_its_start_up_window_says_so() {
    let f = BridgeFacts {
        startup: in_window(),
        ..bridge_running()
    };
    assert_eq!(
        bridge_line(&f),
        "bridge to \"Test \"ASIO\"\": running 96000 Hz / 256 · underruns to ASIO 2 / from ASIO 5 · resets 1 · start-up (first 5 min) · kernel TX"
    );
    let j = bridge_json(&f);
    assert!(
        j.ends_with("\"ktx_failed\":0,\"phase\":\"start-up\"}"),
        "{j}"
    );
    assert!(!j.contains("startup_"), "{j}");
}

#[test]
fn a_bridge_after_its_window_shows_the_boot_s_underruns_apart() {
    // dsg 23.11.2026: the boot's 67 / 37 underruns, a quiet run since.
    let f = BridgeFacts {
        to_asio_underruns: 0,
        from_asio_underruns: 0,
        resets: 0,
        startup: Part {
            in_window: false,
            startup: [67, 37, 0],
        },
        ..bridge_running()
    };
    assert_eq!(
        bridge_line(&f),
        "bridge to \"Test \"ASIO\"\": running 96000 Hz / 256 · underruns to ASIO 0 / from ASIO 0 · resets 0 · at start-up: underruns to ASIO 67, underruns from ASIO 37 · kernel TX"
    );
    let j = bridge_json(&f);
    assert!(
        j.ends_with(concat!(
            "\"ktx_failed\":0,\"phase\":\"running\",\"startup_to_asio_underruns\":67,",
            "\"startup_from_asio_underruns\":37,\"startup_resets\":0}"
        )),
        "{j}"
    );
    assert!(dstack::ctl::app_object(&j).is_some(), "{j}");
    assert_eq!(bridge_state(&f), "ok", "start-up counts never warn");
}
