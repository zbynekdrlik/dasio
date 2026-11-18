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
        rx: None,
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
        late_bins: [0; 8],
        to_asio_converter_delay_ns: 0,
        from_asio_converter_delay_ns: 0,
        pc_problems: Some(0),
        tx: TxPath::default(),
        from_asio_age: None,
        rx: None,
        from_asio_pull: None,
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
        late_bins: [1, 2, 3, 4, 5, 6, 7, 8],
        ..Default::default()
    };
    let f = BridgeFacts::of(
        "Test ASIO",
        &waiting,
        7,
        9,
        Some(2),
        TxPath::Udp,
        Some(AGE_DSG),
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
            late_bins: [1, 2, 3, 4, 5, 6, 7, 8],
            // The players' delays come from their stats, not the status.
            to_asio_converter_delay_ns: 0,
            from_asio_converter_delay_ns: 0,
            pc_problems: Some(2),
            tx: TxPath::Udp,
            from_asio_age: Some(AGE_DSG),
            rx: None,
            from_asio_pull: None,
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

/// dsg's field read (dasio#120, 16.11.2026): 128-frame calls at 96 kHz,
/// the modelled block age 1650 us (max 1888), the TX packets up to 1397 us
/// past their stamps, + 200 us for the network (none measured on Windows).
const AGE_DSG: AgeReport = AgeReport {
    block_age_us: Some(1_650),
    block_age_max_us: Some(1_888),
    packet_age_us: Some(1_397),
    min_receiver_latency_us: Some(1_597),
};

#[test]
fn the_bridge_json_carries_the_from_asio_figures_while_it_has_them() {
    // dstack #325, dasio#120: the four keys with the report's values.
    let j = bridge_json(&BridgeFacts {
        from_asio_age: Some(AGE_DSG),
        ..bridge_running()
    });
    assert!(j.starts_with('{') && j.ends_with('}'), "{j}");
    assert!(
        j.contains(concat!(
            "\"pc_profile_problems\":0,\"from_asio_block_age_us\":1650,",
            "\"from_asio_block_age_max_us\":1888,\"from_asio_packet_age_us\":1397,",
            "\"from_asio_min_receiver_latency_us\":1597,\"kernel_tx\":true,"
        )),
        "{j}"
    );
    // No report (no block taken, no TX packet, or both gone 1-2 min ago):
    // the keys are left out, never a 0 that reads as a fast driver.
    let none = bridge_json(&bridge_running());
    for key in [
        "from_asio_block_age",
        "from_asio_packet_age",
        "from_asio_min_receiver_latency",
    ] {
        assert!(!none.contains(key), "{key}: {none}");
    }
    assert!(
        none.contains("\"pc_profile_problems\":0,\"kernel_tx\":true,"),
        "{none}"
    );
}

#[test]
fn each_from_asio_key_is_left_out_while_its_value_is_unknown() {
    // TX packets but no driver's block taken (no driver, or a closed one):
    // only the packet keys.
    let packets = bridge_json(&BridgeFacts {
        from_asio_age: Some(AgeReport {
            block_age_us: None,
            block_age_max_us: None,
            ..AGE_DSG
        }),
        ..bridge_running()
    });
    assert!(!packets.contains("from_asio_block_age"), "{packets}");
    assert!(
        packets.contains(concat!(
            "\"pc_profile_problems\":0,\"from_asio_packet_age_us\":1397,",
            "\"from_asio_min_receiver_latency_us\":1597,\"kernel_tx\":true,"
        )),
        "{packets}"
    );
    // Blocks taken, no TX packet seen yet: only the block keys.
    let blocks = bridge_json(&BridgeFacts {
        from_asio_age: Some(AgeReport {
            packet_age_us: None,
            min_receiver_latency_us: None,
            ..AGE_DSG
        }),
        ..bridge_running()
    });
    assert!(!blocks.contains("from_asio_packet_age"), "{blocks}");
    assert!(!blocks.contains("from_asio_min_receiver"), "{blocks}");
    assert!(
        blocks.contains(concat!(
            "\"pc_profile_problems\":0,\"from_asio_block_age_us\":1650,",
            "\"from_asio_block_age_max_us\":1888,\"kernel_tx\":true,"
        )),
        "{blocks}"
    );
}

/// A [`BlockAge`] on a clock the test sets, ns.
fn age_on_fake_clock() -> (
    std::sync::Arc<std::sync::atomic::AtomicI64>,
    std::sync::Arc<BlockAge>,
) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};
    let t = Arc::new(AtomicI64::new(0));
    let read = Arc::clone(&t);
    let clock: dlink::bridge::Clock = Arc::new(move || read.load(Ordering::Relaxed));
    (t, Arc::new(BlockAge::new(clock)))
}

/// The TX side's view: the oldest packet since the last view, ns, and one
/// flow to `dsg` at 1 ms (slot 0) with its late count.
fn view(age_peak_ns: u64, dsg_late: u64) -> AppView {
    AppView {
        age_peak_ns,
        receivers: vec![
            dstack::media::tx::ReceiverLateness {
                slot: 0,
                receiver: "dsg".into(),
                latency_ns: 1_000_000,
                over_latency: dsg_late,
            },
            dstack::media::tx::ReceiverLateness {
                slot: 1,
                receiver: "stage-rack".into(),
                latency_ns: 2_000_000,
                over_latency: 0,
            },
        ],
    }
}

#[test]
fn the_age_tick_reports_the_tx_packets_age_plus_the_network() {
    use std::sync::atomic::Ordering;
    let (t, age) = age_on_fake_clock();
    // Before any look (no driver's block, no TX packet): no report.
    assert_eq!(age.report(), None);
    t.store(10_000_000_000, Ordering::Relaxed);
    // dsg's packet peak; no network measured (Windows): 200 us.
    assert_eq!(from_asio_age_tick(&age, &view(1_397_000, 0), None), None);
    let r = age.report().expect("a TX packet was seen");
    assert_eq!(
        (r.packet_age_us, r.min_receiver_latency_us),
        (Some(1_397), Some(1_597))
    );
    // No driver's block taken: the block keys stay unknown.
    assert_eq!((r.block_age_us, r.block_age_max_us), (None, None));
    // The next counts tick, 10 s on: a younger peak and a measured 300 us
    // network p99. The minute's max stays the older packet.
    t.store(20_000_000_000, Ordering::Relaxed);
    from_asio_age_tick(&age, &view(1_003_000, 0), Some(300_000));
    let r = age.report().unwrap();
    assert_eq!(
        (r.packet_age_us, r.min_receiver_latency_us),
        (Some(1_397), Some(1_697))
    );
}

#[test]
fn the_age_tick_warns_once_an_hour_when_a_receiver_counts_late_packets() {
    use std::sync::atomic::Ordering;
    let (t, age) = age_on_fake_clock();
    let tick = 10_000_000_000i64;
    t.store(tick, Ordering::Relaxed);
    // The first look only learns the flows' counts, even a high one.
    assert_eq!(from_asio_age_tick(&age, &view(1_100_000, 4), None), None);
    // No rise: no warning, however young or old the packets.
    t.store(2 * tick, Ordering::Relaxed);
    assert_eq!(from_asio_age_tick(&age, &view(1_397_000, 4), None), None);
    // dsg counted 9 more late packets (its 1 ms self-subscription, 16.11):
    // a warning naming it, the latency it states and the late packets,
    // never the clean 2 ms receiver.
    t.store(3 * tick, Ordering::Relaxed);
    let text = from_asio_age_tick(&age, &view(1_200_000, 13), None).expect("a rise warns");
    assert!(text.contains("dsg at 1000 us (9 late)"), "{text}");
    assert!(!text.contains("stage-rack"), "{text}");
    assert!(text.contains("needs at least 1597 us"), "{text}");
    // A further rise inside the hour: held, no second warning.
    t.store(4 * tick, Ordering::Relaxed);
    assert_eq!(from_asio_age_tick(&age, &view(1_200_000, 15), None), None);
    // An hour after the warning, another rise: one warning, the held rise
    // summed in (2 + 1).
    t.store(3 * tick + 3_600_000_000_000, Ordering::Relaxed);
    let text = from_asio_age_tick(&age, &view(1_200_000, 16), None).expect("an hour on");
    assert!(text.contains("dsg at 1000 us (3 late)"), "{text}");
}

#[test]
fn the_bridge_s_status_loop_checks_the_receivers_once_a_counts_tick() {
    // dstack #325: the look runs in the status loop, never in the pump
    // or the driver's callback (the TX view locks the TX table).
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
    // Never gated on the age's report: only a look records the packet
    // age, so a gated look would never start.
    // The TX view is read there and nowhere else: the device's only
    // reader (a second one would take its peak), and dstack 0.6.151's
    // removed calls are gone.
    let views: Vec<usize> = src
        .match_indices("take_app_view(")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(views.len(), 1, "one TX view in run_bridge.rs");
    assert!(
        (tick..tick_end).contains(&views[0]),
        "the TX view is not read inside the counts tick"
    );
    for gone in ["warn_low_receivers", "receiver_latencies", ".look("] {
        assert_eq!(
            src.matches(gone).count(),
            0,
            "{gone}: only through the tick"
        );
    }
}

#[test]
fn the_rx_arrival_shows_once_measured_and_never_changes_the_icon() {
    use crate::rx_arrival::Report;
    // dasio#128: nothing before a packet was placed.
    assert!(!line(&master()).contains("RX "));
    assert!(!json(&master()).contains("rx_arrival"));
    let measured = Facts {
        rx: Some(Report {
            peak_ns: 7_000_000,
            last: Some((100_000, 350_000)),
            ..Default::default()
        }),
        ..master()
    };
    assert_eq!(
        line(&measured),
        "DAW attached · 64 @ 96 kHz · in 1.67 / out 5.67 ms · late 0/min (95 total) · RX arrival 0.10-0.35 ms, peak 7.00 ms · kernel TX"
    );
    // A measurement, never a judgement: a high peak leaves the icon alone.
    assert_eq!(state(&measured), "ok");
    let j = json(&measured);
    assert!(
        j.contains("\"pc_profile_problems\":-1,\"rx_arrival_peak_ns\":7000000,\"rx_arrival_last_min_ns\":100000,\"rx_arrival_last_max_ns\":350000,\"kernel_tx\":true"),
        "{j}"
    );
    assert!(j.ends_with('}'));
}

#[test]
fn a_bridge_shows_its_rx_arrival_and_never_changes_the_icon() {
    use crate::rx_arrival::Report;
    assert!(!bridge_json(&bridge_running()).contains("rx_arrival"));
    let measured = BridgeFacts {
        rx: Some(Report {
            peak_ns: 3_600_000,
            last: None,
            ..Default::default()
        }),
        ..bridge_running()
    };
    assert!(
        bridge_line(&measured).contains(" · resets 1 · RX arrival peak 3.60 ms · kernel TX"),
        "{}",
        bridge_line(&measured)
    );
    assert_eq!(bridge_state(&measured), "ok");
    let j = bridge_json(&measured);
    assert!(
        j.contains("\"rx_arrival_peak_ns\":3600000,\"kernel_tx\":true"),
        "{j}"
    );
}

#[test]
fn the_rx_arrival_mean_shows_and_only_a_bridge_names_its_asio_pull_wait() {
    use crate::rx_arrival::{Report, Wait};
    let rx = Some(Report {
        peak_ns: 720_000,
        last: Some((100_000, 350_000)),
        mean_ns: Some(210_000),
        wait: Some(Wait {
            mean_ns: Some(1_250_000),
            peak_us: 2_670,
        }),
    });
    // dasio#128: the DAW kind's line and JSON carry the mean; its read
    // wait is no ASIO pull, so no pull wait keys.
    let daw = Facts { rx, ..master() };
    assert!(
        line(&daw).contains(" · RX arrival 0.10-0.35 ms, mean 0.21 ms, peak 0.72 ms · kernel TX"),
        "{}",
        line(&daw)
    );
    let j = json(&daw);
    assert!(
        j.contains("\"rx_arrival_mean_ns\":210000,\"kernel_tx\":true"),
        "{j}"
    );
    assert!(!j.contains("pull_wait"), "{j}");
    assert_eq!(state(&daw), "ok");
    // A bridge's RX writer is the ASIO driver's buffer switch: its wait is
    // the ASIO pull wait, its own keys beside the arrival's.
    let bridge = BridgeFacts {
        rx,
        ..bridge_running()
    };
    assert!(
        bridge_line(&bridge).contains(" · RX arrival 0.10-0.35 ms, mean 0.21 ms, peak 0.72 ms"),
        "{}",
        bridge_line(&bridge)
    );
    let j = bridge_json(&bridge);
    assert!(
        j.contains("\"rx_arrival_mean_ns\":210000,\"rx_asio_pull_wait_mean_ns\":1250000,\"rx_asio_pull_wait_peak_us\":2670,\"kernel_tx\":true"),
        "{j}"
    );
    assert!(j.ends_with('}'));
    assert_eq!(bridge_state(&bridge), "ok");
    // Before a measurement: none of the keys.
    assert!(!bridge_json(&bridge_running()).contains("pull_wait"));
}

fn pull(rebootstraps: u64, skipped: u64, writes_after_send: u64, faults: u64) -> TxFillView {
    TxFillView {
        blocks: 3_000,
        rebootstraps,
        skipped,
        writes_after_send,
        faults,
    }
}

#[test]
fn a_bridge_from_asio_carries_its_pull_s_counts_in_the_json() {
    // dstack v0.6.167 (dasio#120): the TX thread fills the from-ASIO
    // channels; its counts (dstack's `tx_pull`) are the from-ASIO side's
    // health, beside the from-ASIO figures.
    let j = bridge_json(&BridgeFacts {
        from_asio_pull: Some(pull(1, 2, 3, 4)),
        ..bridge_running()
    });
    assert!(
        j.contains(concat!(
            "\"pc_profile_problems\":0,\"from_asio_pull\":{\"blocks\":3000,\"rebootstraps\":1,",
            "\"skipped\":2,\"writes_after_send\":3,\"faults\":4},\"kernel_tx\":true"
        )),
        "{j}"
    );
    assert!(j.ends_with('}'));
    // A bridge with no channel from ASIO has no pull: no key.
    assert!(!bridge_json(&bridge_running()).contains("from_asio_pull"));
}

#[test]
fn a_clean_from_asio_pull_adds_nothing_to_the_line_nor_the_icon() {
    let clean = BridgeFacts {
        from_asio_pull: Some(pull(0, 0, 0, 0)),
        ..bridge_running()
    };
    assert_eq!(bridge_line(&clean), bridge_line(&bridge_running()));
    assert_eq!(bridge_state(&clean), "ok");
}

#[test]
fn each_from_asio_pull_trouble_shows_in_the_line() {
    // Each count alone shows the part (one test a side of the `||`).
    for (p, text) in [
        (
            pull(1, 0, 0, 0),
            " · from ASIO pull: restarts 1, skipped 0, late writes 0, faults 0",
        ),
        (
            pull(0, 7, 0, 0),
            " · from ASIO pull: restarts 0, skipped 7, late writes 0, faults 0",
        ),
        (
            pull(0, 0, 2, 0),
            " · from ASIO pull: restarts 0, skipped 0, late writes 2, faults 0",
        ),
        (
            pull(0, 0, 0, 1),
            " · from ASIO pull: restarts 0, skipped 0, late writes 0, faults 1",
        ),
    ] {
        let f = BridgeFacts {
            from_asio_pull: Some(p),
            ..bridge_running()
        };
        assert!(
            bridge_line(&f).contains(&format!(" · resets 1{text} · kernel TX")),
            "{}",
            bridge_line(&f)
        );
    }
}

#[test]
fn only_a_faulted_from_asio_pull_warns() {
    // A fault silences the from-ASIO channels for good: the icon warns. A
    // restart, a skip or a late write is counted, not a state.
    for (p, state) in [(pull(5, 9, 1, 0), "ok"), (pull(0, 0, 0, 1), "warn")] {
        let f = BridgeFacts {
            from_asio_pull: Some(p),
            ..bridge_running()
        };
        assert_eq!(bridge_state(&f), state, "{p:?}");
    }
}

#[test]
fn the_bridge_instance_hands_the_from_asio_side_to_the_tx_thread() {
    // dstack v0.6.167's consumer port (dstack issue 364): the from-ASIO
    // pull taken before the pump's side, claimed by the TX thread before
    // the device starts, and the pump writes no TX channel.
    // A Windows checkout has CRLF lines: the needles below span lines.
    let src = include_str!("../run_bridge.rs").replace("\r\n", "\n");
    let once = |needle: &str| {
        let found: Vec<usize> = src.match_indices(needle).map(|(i, _)| i).collect();
        assert_eq!(found.len(), 1, "{needle}: once in run_bridge.rs");
        found[0]
    };
    let call = once("pull_from_driver(&mut b,");
    let dante = once(".dante_side()");
    let pump = once("Pump::new(");
    let start = once("prepared\n        .start(");
    let runner = src.find("spawn_runner(").expect("the runner's start");
    assert!(call < dante, "the pull is taken before the pump's side");
    assert!(dante < pump, "the pump's side goes to Pump::new");
    assert!(call < start, "the fill is claimed before the device starts");
    assert!(
        call < runner,
        "the pull is taken before the driver's first open"
    );
    let helper = src.find("\nfn pull_from_driver(").expect("the helper");
    let take = once(".pull_from_asio()");
    let fill = once("TxFill::new(");
    let claim = once(".claim_fill(fill)");
    assert!(
        helper < take && take < fill && fill < claim,
        "take, fill, then claim, in the helper"
    );
    // A refused claim is a start error, never dropped (the pull went with
    // the refused fill: no pump path is left).
    assert!(
        src[claim..].starts_with(
            ".claim_fill(fill)\n        .map_err(|r| format!(\"the from-ASIO pull: {}\", r.reason))?;"
        ),
        "the refusal ends the start"
    );
    let map = src[..pump]
        .rfind("let map = Map {")
        .expect("the pump's map");
    assert!(
        src[map..pump].contains("rx: vec![],\n        tx: vec![],"),
        "the pump's map carries no channel"
    );
    // The pull's counts reach the status.
    assert!(src.contains("from_asio_pull: from_pull.as_ref().map(|p| p.view()),"));
}

/// dstack v0.6.173 / v0.6.174's bridge figures (dasio#120): lateness bins,
/// converter delays, the underrun tick; v0.6.176's network read of them.
mod bridge_figures;

#[test]
fn the_bridge_counts_line_logs_the_peak_lateness_beside_the_bins() {
    // dstack v0.6.178 (dstack issues 393, 399): the run's peak lateness
    // behind the driver's schedule goes in the counts line right after the
    // bins, so the top bin's reach is read beside its count.
    let src = include_str!("../run_bridge.rs").replace("\r\n", "\n");
    let bins = src
        .find("late_bins = ?s.late_bins,\n")
        .expect("the bins in the counts line");
    let peak = src
        .find("late_max_us = s.late_max_us,\n")
        .expect("the peak lateness in the counts line");
    assert!(bins < peak && peak - bins < 80, "right after the bins");
    let line = src[..bins].rfind("info!(").expect("the counts line");
    assert!(src[line..bins].contains("late_callbacks = s.late_callbacks,"));
}
