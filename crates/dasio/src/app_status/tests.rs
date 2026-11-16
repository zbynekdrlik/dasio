use super::*;

fn master() -> Facts {
    Facts {
        attached: true,
        block: 64,
        sample_rate: 96_000,
        latency_in: 160,
        latency_out: 544,
        spare_blocks: 0,
        late_blocks: 95,
        driver_missed: 40,
        stale: 2,
        waited: 70,
        heartbeat_ms: 1234,
        late_per_min: 0,
        pc_problems: None,
        tx: TxPath::default(),
    }
}

#[test]
fn the_line_says_the_daw_the_block_the_latencies_and_the_late_blocks() {
    assert_eq!(
        line(&master()),
        "DAW attached · 64 @ 96 kHz · in 1.67 / out 5.67 ms · late 0/min (95 total) · kernel TX"
    );
    let band = Facts {
        block: 256,
        spare_blocks: 1,
        latency_in: 352,
        latency_out: 864,
        attached: false,
        late_per_min: 3,
        ..master()
    };
    assert_eq!(
        line(&band),
        "no DAW · 256 @ 96 kHz + spare · in 3.67 / out 9.00 ms · late 3/min (95 total) · kernel TX"
    );
}

#[test]
fn the_state_warns_without_a_daw_or_on_a_late_block() {
    assert_eq!(state(&master()), "ok");
    assert_eq!(
        state(&Facts {
            attached: false,
            ..master()
        }),
        "warn"
    );
    assert_eq!(
        state(&Facts {
            late_per_min: 1,
            ..master()
        }),
        "warn"
    );
}

#[test]
fn the_json_is_one_object_with_every_field() {
    let j = json(&master());
    assert!(j.starts_with('{') && j.ends_with('}'), "{j}");
    for key in [
        "\"line\":\"DAW attached",
        "\"state\":\"ok\"",
        "\"daw_attached\":true",
        "\"block\":64",
        "\"sample_rate\":96000",
        "\"latency_in\":160",
        "\"latency_out\":544",
        "\"spare_blocks\":0",
        "\"late_blocks\":95",
        "\"late_per_min\":0",
        "\"driver_missed\":40",
        "\"stale\":2",
        "\"waited\":70",
        "\"heartbeat_ms\":1234",
        "\"pc_profile_problems\":-1",
    ] {
        assert!(j.contains(key), "{key} missing in {j}");
    }
    assert!(j.len() <= 4096, "within dstack's APP_MAX");
}

#[test]
fn the_window_counts_the_last_minute_and_starts_again_after_a_restart() {
    let mut w = LateWindow::default();
    assert_eq!(w.take(0, 10), 0);
    assert_eq!(w.take(30, 13), 3);
    assert_eq!(
        w.take(LATE_WINDOW_S, 13),
        3,
        "a reading a window old still counts"
    );
    assert_eq!(w.take(LATE_WINDOW_S + 1, 13), 0, "then it falls out");
    assert_eq!(w.take(LATE_WINDOW_S + 2, 2), 0, "a restarted counter");
    assert_eq!(w.take(LATE_WINDOW_S + 3, 4), 2);
}

#[test]
fn a_pc_profile_problem_shows_in_the_line_and_warns() {
    let ok = Facts {
        pc_problems: Some(0),
        ..master()
    };
    assert_eq!(
        line(&ok),
        line(&master()),
        "an as-it-should-be PC adds nothing"
    );
    assert_eq!(state(&ok), "ok");
    let one = Facts {
        pc_problems: Some(1),
        ..master()
    };
    assert!(
        line(&one).ends_with(" · PC profile: 1 problem"),
        "{}",
        line(&one)
    );
    assert_eq!(state(&one), "warn");
    let three = Facts {
        pc_problems: Some(3),
        ..master()
    };
    assert!(line(&three).ends_with(" · PC profile: 3 problems"));
    assert!(json(&three).contains("\"pc_profile_problems\":3"));
}

fn bridge_running() -> BridgeFacts {
    BridgeFacts {
        driver: "Test \"ASIO\"".into(),
        running: true,
        reason: None,
        reason_code: None,
        retry_in_s: None,
        rate: 96_000,
        buffer_frames: 256,
        to_asio_underruns: 2,
        from_asio_underruns: 5,
        resets: 1,
        pc_problems: Some(0),
        tx: TxPath::default(),
        from_asio_age: None,
    }
}

#[test]
fn the_bridge_facts_come_from_the_runner_s_status() {
    let waiting = dstack_asio::Status {
        state: dstack_asio::RunState::Waiting,
        reason: Some(dstack_asio::Reason::NoClock),
        retry_in_s: Some(2.0),
        rate: 48_000,
        buffer_frames: 128,
        resets: 4,
        ..Default::default()
    };
    let f = BridgeFacts::of(
        "Test ASIO",
        &waiting,
        7,
        9,
        Some(2),
        TxPath::Udp,
        Some(AGE_128_96K),
    );
    assert_eq!(
        f,
        BridgeFacts {
            driver: "Test ASIO".into(),
            running: false,
            reason: Some(dstack_asio::Reason::NoClock.to_string()),
            reason_code: Some(dstack_asio::Reason::NoClock.code()),
            retry_in_s: Some(2.0),
            rate: 48_000,
            buffer_frames: 128,
            to_asio_underruns: 7,
            from_asio_underruns: 9,
            resets: 4,
            pc_problems: Some(2),
            tx: TxPath::Udp,
            from_asio_age: Some(AGE_128_96K),
        }
    );
    let running = dstack_asio::Status {
        state: dstack_asio::RunState::Running,
        ..Default::default()
    };
    let f = BridgeFacts::of("Test ASIO", &running, 0, 0, None, TxPath::default(), None);
    assert!(f.running);
    assert_eq!((f.reason, f.reason_code), (None, None));
}

#[test]
fn a_running_bridge_says_its_rate_buffer_and_underruns() {
    let f = bridge_running();
    assert_eq!(
        bridge_line(&f),
        "bridge to \"Test \"ASIO\"\": running 96000 Hz / 256 · underruns to ASIO 2 / from ASIO 5 · resets 1 · kernel TX"
    );
    assert_eq!(bridge_state(&f), "ok");
}

#[test]
fn a_waiting_bridge_says_why_and_when_it_tries_again() {
    let f = BridgeFacts {
        running: false,
        reason: Some("the driver gives no clock".into()),
        reason_code: Some("no_clock"),
        retry_in_s: Some(9.6),
        ..bridge_running()
    };
    assert!(
        bridge_line(&f).contains(": waiting (the driver gives no clock), again in 10 s · "),
        "{}",
        bridge_line(&f)
    );
    assert_eq!(bridge_state(&f), "warn");
    let opening = BridgeFacts {
        running: false,
        ..bridge_running()
    };
    assert!(
        bridge_line(&opening).contains(": waiting (opening) · "),
        "{}",
        bridge_line(&opening)
    );
}

#[test]
fn the_bridge_json_is_one_object_with_every_field_escaped() {
    let j = bridge_json(&BridgeFacts {
        reason_code: Some("reset"),
        ..bridge_running()
    });
    assert!(j.starts_with('{') && j.ends_with('}'), "{j}");
    for key in [
        "\"line\":\"bridge to \\\"Test \\\"ASIO\\\"\\\": running",
        "\"state\":\"ok\"",
        "\"kind\":\"bridge\"",
        "\"daw_attached\":false",
        "\"driver\":\"Test \\\"ASIO\\\"\"",
        "\"running\":true",
        "\"reason\":\"reset\"",
        "\"rate\":96000",
        "\"buffer_frames\":256",
        "\"to_asio_underruns\":2",
        "\"from_asio_underruns\":5",
        "\"resets\":1",
        "\"pc_profile_problems\":0",
    ] {
        assert!(j.contains(key), "{key} missing in {j}");
    }
    assert!(bridge_json(&bridge_running()).contains("\"reason\":\"\""));
    let slash = bridge_json(&BridgeFacts {
        driver: "a\\b".into(),
        ..bridge_running()
    });
    assert!(slash.contains("\"driver\":\"a\\\\b\""), "{slash}");
}

#[test]
fn control_characters_are_escaped_in_the_json() {
    assert_eq!(
        escaped("a\nb\rc\td\u{1}e\u{1f}f \u{7f}"),
        "a\\nb\\rc\\td\\u0001e\\u001ff \u{7f}"
    );
    assert_eq!(escaped("\"\\"), "\\\"\\\\");
}

#[test]
fn a_bridge_with_pc_profile_problems_says_so_and_warns() {
    // dasio#120: the bridge checks the profile in process; its tray line and
    // state show the problems as the DAW kind's do.
    let two = BridgeFacts {
        pc_problems: Some(2),
        ..bridge_running()
    };
    assert!(
        bridge_line(&two).ends_with(" · resets 1 · kernel TX · PC profile: 2 problems"),
        "{}",
        bridge_line(&two)
    );
    assert_eq!(bridge_state(&two), "warn");
    assert!(bridge_json(&two).contains("\"pc_profile_problems\":2"));
    let one = BridgeFacts {
        pc_problems: Some(1),
        ..bridge_running()
    };
    assert!(bridge_line(&one).ends_with(" · PC profile: 1 problem"));
    let unchecked = BridgeFacts {
        pc_problems: None,
        ..bridge_running()
    };
    assert!(bridge_line(&unchecked).ends_with(" · resets 1 · kernel TX"));
    assert_eq!(bridge_state(&unchecked), "ok");
    assert!(bridge_json(&unchecked).contains("\"pc_profile_problems\":-1"));
}

#[test]
fn the_tx_part_says_kernel_tx_or_what_missed_the_filter() {
    // dasio#125: the tray shows how audio TX goes out.
    assert!(line(&master()).ends_with(" · kernel TX"));
    let missed = Facts {
        tx: TxPath::Kernel {
            no_slot: 3,
            failed: 1,
        },
        ..master()
    };
    assert!(
        line(&missed).ends_with(" · kernel TX: 3 to the socket, 1 failed (10 s)"),
        "{}",
        line(&missed)
    );
    assert_eq!(state(&missed), "warn");
    for (no_slot, failed) in [(1, 0), (0, 1)] {
        assert_eq!(
            state(&Facts {
                tx: TxPath::Kernel { no_slot, failed },
                ..master()
            }),
            "warn"
        );
    }
    let udp = Facts {
        tx: TxPath::Udp,
        ..master()
    };
    assert!(line(&udp).ends_with(" · UDP TX (test clock)"));
    assert_eq!(state(&udp), "ok");
    let with_pc = Facts {
        pc_problems: Some(1),
        ..master()
    };
    assert!(line(&with_pc).ends_with(" · kernel TX · PC profile: 1 problem"));
}

#[test]
fn the_json_carries_the_tx_path() {
    assert!(json(&master()).contains("\"kernel_tx\":true,\"ktx_no_slot\":0,\"ktx_failed\":0"));
    let missed = Facts {
        tx: TxPath::Kernel {
            no_slot: 4,
            failed: 2,
        },
        ..master()
    };
    assert!(json(&missed).contains("\"kernel_tx\":true,\"ktx_no_slot\":4,\"ktx_failed\":2"));
    let udp = json(&Facts {
        tx: TxPath::Udp,
        ..master()
    });
    assert!(udp.contains("\"kernel_tx\":false"), "{udp}");
    assert!(udp.ends_with('}'));
}

#[test]
fn a_bridge_shows_its_tx_path_and_warns_on_missed_frames() {
    let missed = BridgeFacts {
        tx: TxPath::Kernel {
            no_slot: 0,
            failed: 5,
        },
        ..bridge_running()
    };
    assert!(bridge_line(&missed).contains(" · kernel TX: 0 to the socket, 5 failed (10 s)"));
    assert_eq!(bridge_state(&missed), "warn");
    assert!(bridge_json(&missed).contains("\"ktx_failed\":5"));
    let udp = BridgeFacts {
        tx: TxPath::Udp,
        ..bridge_running()
    };
    assert!(bridge_line(&udp).ends_with(" · resets 1 · UDP TX (test clock)"));
    assert_eq!(bridge_state(&udp), "ok");
}

/// dsg's case (dasio#120): 128-frame calls at 96 kHz (1333 us), the pump
/// taking 200 us after the hand-over, a pump block of 1333 us on top.
const AGE_128_96K: AgeReport = AgeReport {
    block_age_us: 1_533,
    block_age_max_us: 1_600,
    min_receiver_latency_us: 2_933,
};

#[test]
fn the_bridge_json_carries_the_from_asio_block_age_while_it_has_one() {
    // dstack #325, dasio#120: the three keys with the report's values.
    let j = bridge_json(&BridgeFacts {
        from_asio_age: Some(AGE_128_96K),
        ..bridge_running()
    });
    assert!(j.starts_with('{') && j.ends_with('}'), "{j}");
    assert!(
        j.contains(concat!(
            "\"pc_profile_problems\":0,\"from_asio_block_age_us\":1533,",
            "\"from_asio_block_age_max_us\":1600,",
            "\"from_asio_min_receiver_latency_us\":2933,\"kernel_tx\":true,"
        )),
        "{j}"
    );
    // No report (no take yet, or the driver stopped 1-2 min ago): the keys
    // are left out, never a 0 that reads as a fast driver.
    let none = bridge_json(&bridge_running());
    assert!(!none.contains("from_asio_block_age"), "{none}");
    assert!(!none.contains("from_asio_min_receiver_latency"), "{none}");
    assert!(
        none.contains("\"pc_profile_problems\":0,\"kernel_tx\":true,"),
        "{none}"
    );
}

/// A [`BlockAge`] for a 1333 us pump block on a clock the test sets, ns.
fn age_on_fake_clock() -> (
    std::sync::Arc<std::sync::atomic::AtomicI64>,
    std::sync::Arc<BlockAge>,
) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};
    let t = Arc::new(AtomicI64::new(0));
    let read = Arc::clone(&t);
    let clock: dlink::bridge::Clock = Arc::new(move || read.load(Ordering::Relaxed));
    (t, Arc::new(BlockAge::new(1_333_333, clock)))
}

#[test]
fn the_age_tick_reads_no_receivers_before_the_first_take() {
    // No driver serving: the status loop takes no lock on the TX table.
    let (_, age) = age_on_fake_clock();
    let mut read = false;
    let warned = from_asio_age_tick(&age, || {
        read = true;
        vec![("rx".into(), 1_000_000)]
    });
    assert_eq!(warned, None);
    assert!(!read, "the receivers were read without an age report");
}

#[test]
fn the_age_tick_warns_about_a_1_ms_receiver_once_an_hour() {
    use std::sync::atomic::Ordering;
    let (t, age) = age_on_fake_clock();
    t.store(5_000_000, Ordering::Relaxed);
    age.handed(128, 96_000);
    t.store(5_200_000, Ordering::Relaxed);
    age.taken();
    let receivers = || {
        vec![
            ("stage-rack".to_string(), 1_000_000),
            ("unknown".to_string(), 0),
            ("foh".to_string(), 5_000_000),
        ]
    };
    let text = from_asio_age_tick(&age, receivers).expect("a 1 ms receiver warns");
    assert!(text.contains("stage-rack at 1000 us"), "{text}");
    assert!(!text.contains("foh") && !text.contains("unknown"), "{text}");
    // The next counts tick, 10 s later: the receivers are read, no new
    // warning.
    t.store(10_005_200_000, Ordering::Relaxed);
    let mut read = false;
    let again = from_asio_age_tick(&age, || {
        read = true;
        receivers()
    });
    assert_eq!(again, None);
    assert!(read);
    // An hour after the warning, with blocks still taken: it warns again.
    let hour_later = 5_200_000 + 3_600_000_000_000;
    t.store(hour_later - 200_000, Ordering::Relaxed);
    age.handed(128, 96_000);
    t.store(hour_later, Ordering::Relaxed);
    age.taken();
    assert!(from_asio_age_tick(&age, receivers).is_some());
    // Receivers at or above the need (2866 us: 1533 us of age + a 1333 us
    // pump block): nothing.
    let (t, age) = age_on_fake_clock();
    t.store(5_000_000, Ordering::Relaxed);
    age.handed(128, 96_000);
    t.store(5_200_000, Ordering::Relaxed);
    age.taken();
    assert_eq!(
        from_asio_age_tick(&age, || vec![
            ("at-need".into(), 2_866_000),
            ("foh".into(), 3_000_000)
        ]),
        None
    );
}

#[test]
fn the_bridge_s_status_loop_checks_the_receivers_once_a_counts_tick() {
    // dstack #325: the check runs in the status loop, never in the pump
    // or the driver's callback (it locks the TX table and allocates).
    let src = include_str!("../run_bridge.rs");
    let calls: Vec<usize> = src
        .match_indices("from_asio_age_tick(")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(calls.len(), 1, "one call in run_bridge.rs");
    let tick = src.find("if counts.due(now) {").expect("the counts tick");
    let tick_end = src[tick..].find("\"bridge counts\"").expect("its log line") + tick;
    assert!(
        (tick..tick_end).contains(&calls[0]),
        "the call is not inside the counts tick"
    );
    let callback = src
        .find("let make: MakeProcess")
        .expect("the driver's side");
    assert!(
        calls[0] < callback,
        "the call is in run(), not in the driver's side"
    );
    assert_eq!(
        src.matches("warn_low_receivers").count(),
        0,
        "only through the tick"
    );
}
