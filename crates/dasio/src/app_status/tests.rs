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
    }
}

#[test]
fn the_line_says_the_daw_the_block_the_latencies_and_the_late_blocks() {
    assert_eq!(
        line(&master()),
        "DAW attached · 64 @ 96 kHz · in 1.67 / out 5.67 ms · late 0/min (95 total)"
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
        "no DAW · 256 @ 96 kHz + spare · in 3.67 / out 9.00 ms · late 3/min (95 total)"
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
    let f = BridgeFacts::of("Test ASIO", &waiting, 7, 9);
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
        }
    );
    let running = dstack_asio::Status {
        state: dstack_asio::RunState::Running,
        ..Default::default()
    };
    let f = BridgeFacts::of("Test ASIO", &running, 0, 0);
    assert!(f.running);
    assert_eq!((f.reason, f.reason_code), (None, None));
}

#[test]
fn a_running_bridge_says_its_rate_buffer_and_underruns() {
    let f = bridge_running();
    assert_eq!(
        bridge_line(&f),
        "bridge to \"Test \"ASIO\"\": running 96000 Hz / 256 · underruns to ASIO 2 / from ASIO 5 · resets 1"
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
