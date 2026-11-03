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
