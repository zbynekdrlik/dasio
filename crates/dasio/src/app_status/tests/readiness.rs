//! One look at what keeps a PC from low latency (dasio#237, owner
//! 25.11.2026: dasio's own checks bring a new PC there in one pass): the PC
//! profile, the TX path and the clock sign, each with its number. Only a
//! failing item shows in the tray line; the JSON says `ok`, `not` or
//! `checking`. The clock sign never sets the icon (rx-arrival.md).

use super::*;

fn rx_mean(mean_ns: u64) -> Option<crate::rx_arrival::Report> {
    Some(crate::rx_arrival::Report {
        peak_ns: 700_000,
        last: Some((100_000, 350_000)),
        mean_ns: Some(mean_ns),
        wait: None,
    })
}

fn ready_daw() -> Facts {
    Facts {
        pc_problems: Some(0),
        rx: rx_mean(190_000),
        ..master()
    }
}

#[test]
fn a_pc_with_every_item_holding_is_ok_and_its_line_is_unchanged() {
    let f = ready_daw();
    assert_eq!(readiness(f.pc_problems, f.tx, f.rx, None), Readiness::Ok);
    assert!(!line(&f).contains("low latency"), "{}", line(&f));
    assert!(json(&f).contains("\"low_latency\":\"ok\","), "{}", json(&f));
}

#[test]
fn a_pc_profile_problem_is_named_with_its_count() {
    let f = Facts {
        pc_problems: Some(2),
        ..ready_daw()
    };
    assert_eq!(
        readiness(f.pc_problems, f.tx, f.rx, None),
        Readiness::Not(vec!["PC profile (2)".to_string()])
    );
    assert!(
        line(&f)
            .contains(" · late 0/min (95 total) · low latency NOT: PC profile (2) · RX arrival "),
        "{}",
        line(&f)
    );
    assert!(
        json(&f).contains("\"low_latency\":\"not\","),
        "{}",
        json(&f)
    );
}

#[test]
fn no_pc_profile_is_no_item() {
    let f = Facts {
        pc_problems: None,
        ..ready_daw()
    };
    assert_eq!(readiness(f.pc_problems, f.tx, f.rx, None), Readiness::Ok);
}

#[test]
fn frames_that_missed_the_filter_and_the_udp_path_are_tx_items() {
    let missed = TxPath::Kernel {
        no_slot: 3,
        failed: 1,
    };
    assert_eq!(
        readiness(Some(0), missed, rx_mean(190_000), None),
        Readiness::Not(vec!["TX (3 to the socket, 1 failed)".to_string()])
    );
    let only_failed = TxPath::Kernel {
        no_slot: 0,
        failed: 2,
    };
    assert_eq!(
        readiness(Some(0), only_failed, rx_mean(190_000), None),
        Readiness::Not(vec!["TX (0 to the socket, 2 failed)".to_string()])
    );
    let only_socket = TxPath::Kernel {
        no_slot: 5,
        failed: 0,
    };
    assert_eq!(
        readiness(Some(0), only_socket, rx_mean(190_000), None),
        Readiness::Not(vec!["TX (5 to the socket, 0 failed)".to_string()])
    );
    assert_eq!(
        readiness(Some(0), TxPath::Udp, rx_mean(190_000), None),
        Readiness::Not(vec!["TX (UDP)".to_string()])
    );
}

#[test]
fn an_rx_mean_under_the_clock_sign_is_the_clock_item() {
    // alex-nb 26.11.2026: 0.02 ms behind its USB NIC's lagging follower
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(20_000), None),
        Readiness::Not(vec!["clock (RX mean 0.02 ms)".to_string()])
    );
    assert_eq!(
        readiness(
            Some(0),
            TxPath::default(),
            rx_mean(CLOCK_SIGN_MEAN_NS - 1),
            None
        ),
        Readiness::Not(vec!["clock (RX mean 0.05 ms)".to_string()])
    );
    // the sign itself and FOH's 0.18 ms hold
    assert_eq!(
        readiness(
            Some(0),
            TxPath::default(),
            rx_mean(CLOCK_SIGN_MEAN_NS),
            None
        ),
        Readiness::Ok
    );
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(180_000), None),
        Readiness::Ok
    );
}

#[test]
fn every_failing_item_is_listed_in_order() {
    let f = Facts {
        pc_problems: Some(1),
        tx: TxPath::Udp,
        rx: rx_mean(0),
        ..master()
    };
    assert_eq!(
        readiness(f.pc_problems, f.tx, f.rx, None),
        Readiness::Not(vec![
            "PC profile (1)".to_string(),
            "TX (UDP)".to_string(),
            "clock (RX mean 0.00 ms)".to_string(),
        ])
    );
    assert!(
        line(&f).contains(
            " · low latency NOT: PC profile (1), TX (UDP), clock (RX mean 0.00 ms) · RX arrival "
        ),
        "{}",
        line(&f)
    );
}

#[test]
fn no_rx_mean_yet_is_checking_and_a_failing_item_still_shows() {
    let none = Facts {
        rx: None,
        ..ready_daw()
    };
    assert_eq!(
        readiness(none.pc_problems, none.tx, none.rx, None),
        Readiness::Checking
    );
    assert!(!line(&none).contains("low latency"), "{}", line(&none));
    assert!(
        json(&none).contains("\"low_latency\":\"checking\","),
        "{}",
        json(&none)
    );
    let tick_without_packet = Some(crate::rx_arrival::Report {
        peak_ns: 700_000,
        last: None,
        mean_ns: None,
        wait: None,
    });
    assert_eq!(
        readiness(Some(0), TxPath::default(), tick_without_packet, None),
        Readiness::Checking
    );
    assert_eq!(
        readiness(Some(3), TxPath::default(), None, None),
        Readiness::Not(vec!["PC profile (3)".to_string()])
    );
}

#[test]
fn the_clock_sign_never_sets_the_icon() {
    let lagging = Facts {
        rx: rx_mean(20_000),
        ..ready_daw()
    };
    assert!(line(&lagging).contains("low latency NOT: clock (RX mean 0.02 ms)"));
    assert_eq!(state(&lagging), "ok");
    let bridge = BridgeFacts {
        rx: rx_mean(20_000),
        ..bridge_running()
    };
    assert_eq!(bridge_state(&bridge), "ok");
}

#[test]
fn a_bridge_names_its_failing_items_before_its_underruns() {
    let f = BridgeFacts {
        pc_problems: Some(1),
        rx: rx_mean(20_000),
        ..bridge_running()
    };
    assert!(
        bridge_line(&f).contains(
            ": running 96000 Hz / 256 · low latency NOT: PC profile (1), clock (RX mean 0.02 ms) · underruns to ASIO 2"
        ),
        "{}",
        bridge_line(&f)
    );
    let j = bridge_json(&f);
    assert!(
        j.contains("\"state\":\"warn\",\"low_latency\":\"not\",\"kind\":\"bridge\","),
        "{j}"
    );
    let ready = BridgeFacts {
        rx: rx_mean(190_000),
        ..bridge_running()
    };
    assert!(!bridge_line(&ready).contains("low latency"));
    assert!(bridge_json(&ready).contains("\"low_latency\":\"ok\","));
    assert!(bridge_json(&bridge_running()).contains("\"low_latency\":\"checking\","));
}

fn age(receiver: &str, age_us: u64, latency_us: u64) -> Option<TxAge> {
    Some(TxAge {
        receiver: ShortName::new(receiver),
        age_ns: age_us * 1_000,
        edge_ns: latency_us * 1_000,
    })
}

#[test]
fn a_tx_lane_older_than_its_receiver_s_latency_is_the_tx_age_item() {
    // a packet 1021 us old at an edge of 250 us
    assert_eq!(
        readiness(
            Some(0),
            TxPath::default(),
            rx_mean(190_000),
            age("foh-abl-yamaha-accelerator", 1_021, 250)
        ),
        Readiness::Not(vec![
            "TX age 1.02 ms past the 0.25 ms foh-abl-yamaha-accelerator plays it at".to_string()
        ])
    );
    // at the latency itself the packet still plays (dstack's over_latency)
    assert_eq!(
        readiness(
            Some(0),
            TxPath::default(),
            rx_mean(190_000),
            age("iem-mixer", 1_000, 1_000)
        ),
        Readiness::Ok
    );
    assert_eq!(
        readiness(
            Some(0),
            TxPath::default(),
            rx_mean(190_000),
            age("iem-mixer", 1_001, 1_000)
        ),
        Readiness::Not(vec![
            "TX age 1.00 ms past the 1.00 ms iem-mixer plays it at".to_string()
        ])
    );
    // no measured lane is no item, and nothing to wait for
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(190_000), None),
        Readiness::Ok
    );
}

#[test]
fn the_tx_age_item_comes_after_the_tx_path_and_before_the_clock() {
    let f = Facts {
        tx: TxPath::Udp,
        rx: rx_mean(20_000),
        tx_age: age("band", 2_000, 1_000),
        ..ready_daw()
    };
    assert_eq!(
        readiness(f.pc_problems, f.tx, f.rx, f.tx_age),
        Readiness::Not(vec![
            "TX (UDP)".to_string(),
            "TX age 2.00 ms past the 1.00 ms band plays it at".to_string(),
            "clock (RX mean 0.02 ms)".to_string(),
        ])
    );
    assert!(
        line(&f).contains(
            "low latency NOT: TX (UDP), TX age 2.00 ms past the 1.00 ms band plays it at, clock"
        ),
        "{}",
        line(&f)
    );
    assert!(
        json(&f).contains("\"low_latency\":\"not\","),
        "{}",
        json(&f)
    );
    // the icon keeps its own reasons only: UDP sets it, the age does not
    let aged = Facts {
        tx_age: age("band", 2_000, 1_000),
        ..ready_daw()
    };
    assert_eq!(state(&aged), "ok");
}

#[test]
fn the_worst_lane_is_the_one_furthest_past_its_receiver_s_latency() {
    // 3 ms of 4 ms (0.75) is nearer its receiver's latency than 1 ms of 2 ms (0.5)
    let w = worst_tx_age([
        ("iem-mixer", 2_000_000, Some(1_000_000)),
        ("band", 4_000_000, Some(3_000_000)),
        ("zvukar", 1_000_000, Some(400_000)),
    ])
    .unwrap();
    assert_eq!(
        (w.receiver.as_str(), w.age_ns, w.edge_ns),
        ("band", 3_000_000, 4_000_000)
    );
    // a later lane only replaces it when it is further, not as far
    let tie = worst_tx_age([("a", 2_000, Some(1_000)), ("b", 4_000, Some(2_000))]).unwrap();
    assert_eq!(tie.receiver.as_str(), "a");
    let past = worst_tx_age([("a", 2_000, Some(1_000)), ("b", 4_000, Some(2_001))]).unwrap();
    assert_eq!(past.receiver.as_str(), "b");
}

#[test]
fn a_lane_without_one_receiver_a_latency_or_a_window_does_not_count() {
    assert_eq!(worst_tx_age([]), None);
    assert_eq!(worst_tx_age([("", 1_000, Some(5_000))]), None);
    assert_eq!(worst_tx_age([("band", 0, Some(5_000))]), None);
    assert_eq!(worst_tx_age([("band", 1_000, None)]), None);
    let w = worst_tx_age([
        ("", 1_000, Some(9_000)),
        ("band", 1_000, Some(0)),
        ("multicast", 0, Some(9_000)),
    ])
    .unwrap();
    assert_eq!((w.receiver.as_str(), w.age_ns), ("band", 0));
}

#[test]
fn a_receiver_name_keeps_31_bytes_and_cuts_at_a_char_boundary() {
    assert_eq!(ShortName::new("iem-mixer").as_str(), "iem-mixer");
    assert_eq!(ShortName::new("").as_str(), "");
    let long = "a".repeat(40);
    assert_eq!(ShortName::new(&long).as_str(), &long[..31]);
    let exact = "b".repeat(31);
    assert_eq!(ShortName::new(&exact).as_str(), exact);
    // 30 bytes then a 2-byte char: the char would end at byte 32, so it goes
    let wide = format!("{}é", "c".repeat(30));
    assert_eq!(ShortName::new(&wide).as_str(), "c".repeat(30));
    let fits = format!("{}é", "d".repeat(29));
    assert_eq!(ShortName::new(&fits).as_str(), fits);
}

#[test]
fn the_worst_lane_is_by_ratio_not_by_age_or_by_excess() {
    // a 5 ms packet at a 20 ms receiver (0.25) is older but further from late
    // than a 3 ms packet at a 4 ms receiver (0.75)
    let w = worst_tx_age([
        ("far", 20_000_000, Some(5_000_000)),
        ("band", 4_000_000, Some(3_000_000)),
    ])
    .unwrap();
    assert_eq!(w.receiver.as_str(), "band");
    // past the latency: 3 ms at 1 ms (3.0, 2 ms over) before 10 ms at 7 ms
    // (1.43, 3 ms over)
    let w = worst_tx_age([
        ("long", 7_000_000, Some(10_000_000)),
        ("short", 1_000_000, Some(3_000_000)),
    ])
    .unwrap();
    assert_eq!(w.receiver.as_str(), "short");
}

#[test]
fn the_tx_age_item_waits_for_the_start_up_window() {
    let starting = Facts {
        tx_age: age("band", 2_000, 1_000),
        startup: Part {
            in_window: true,
            startup: [0; 4],
        },
        ..ready_daw()
    };
    assert!(!line(&starting).contains("TX age"), "{}", line(&starting));
    assert!(
        json(&starting).contains("\"low_latency\":\"ok\","),
        "{}",
        json(&starting)
    );
    let running = Facts {
        startup: Part::default(),
        ..starting
    };
    assert!(
        line(&running)
            .contains("low latency NOT: TX age 2.00 ms past the 1.00 ms band plays it at")
    );
}

#[test]
fn a_tx_age_window_no_late_watch_feeds_does_not_count() {
    let mut seen = (0, 0);
    // no look yet: nothing to read
    assert!(!sub_fed(&mut seen, 0));
    // the looks move: fed
    assert!(sub_fed(&mut seen, 1));
    assert!(sub_fed(&mut seen, 2));
    // standing still a few calls is timing
    for _ in 1..TX_AGE_STALE_READS {
        assert!(sub_fed(&mut seen, 2));
    }
    // then it is a dead watch
    assert!(!sub_fed(&mut seen, 2));
    assert!(!sub_fed(&mut seen, 2));
    // and a moving count is fed again at once
    assert!(sub_fed(&mut seen, 3));
    assert_eq!(seen, (3, 0));
}

fn sub_fed(seen: &mut (u64, u32), looks: u64) -> bool {
    tx_age_fed(seen, looks)
}

fn lane(
    receiver: &str,
    receiver_us: u64,
    threshold_us: Option<u64>,
    age_us: Option<u64>,
) -> dstack::media::tx::TxAgeLane {
    dstack::media::tx::TxAgeLane {
        slot: 0,
        receiver: receiver.to_string(),
        receiver_latency_ns: receiver_us * 1_000,
        age_max_10s_ns: age_us.map(|a| a * 1_000),
        over_latency: 0,
        late_threshold_ns: threshold_us.map(|t| t * 1_000),
    }
}

#[test]
fn a_lane_is_judged_at_dstack_s_late_edge_not_at_its_receiver_s_latency() {
    // FOH band 26.11.2026 09:05Z: 0.34 ms at iem-mixer, which states
    // 0.25 ms but plays band's flow at band's 1 ms TX latency plus a packet
    // and the margin: dstack judges it at ~1.11 ms, so it plays (dasio#237)
    let foh = worst_lane(&[lane("iem-mixer", 250, Some(1_113), Some(340))]);
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(190_000), foh),
        Readiness::Ok
    );
    // past that edge it is the item
    let late = worst_lane(&[lane("iem-mixer", 250, Some(1_113), Some(1_200))]);
    assert!(
        matches!(
            readiness(Some(0), TxPath::default(), rx_mean(190_000), late),
            Readiness::Not(_)
        ),
        "{late:?}"
    );
    // no edge (no flow, or the receiver's latency unknown): no item
    assert_eq!(
        worst_lane(&[lane("iem-mixer", 250, None, Some(5_000))]),
        None
    );
}
