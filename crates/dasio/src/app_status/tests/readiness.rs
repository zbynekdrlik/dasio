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
    assert_eq!(readiness(f.pc_problems, f.tx, f.rx), Readiness::Ok);
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
        readiness(f.pc_problems, f.tx, f.rx),
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
    assert_eq!(readiness(f.pc_problems, f.tx, f.rx), Readiness::Ok);
}

#[test]
fn frames_that_missed_the_filter_and_the_udp_path_are_tx_items() {
    let missed = TxPath::Kernel {
        no_slot: 3,
        failed: 1,
    };
    assert_eq!(
        readiness(Some(0), missed, rx_mean(190_000)),
        Readiness::Not(vec!["TX (3 to the socket, 1 failed)".to_string()])
    );
    let only_failed = TxPath::Kernel {
        no_slot: 0,
        failed: 2,
    };
    assert_eq!(
        readiness(Some(0), only_failed, rx_mean(190_000)),
        Readiness::Not(vec!["TX (0 to the socket, 2 failed)".to_string()])
    );
    let only_socket = TxPath::Kernel {
        no_slot: 5,
        failed: 0,
    };
    assert_eq!(
        readiness(Some(0), only_socket, rx_mean(190_000)),
        Readiness::Not(vec!["TX (5 to the socket, 0 failed)".to_string()])
    );
    assert_eq!(
        readiness(Some(0), TxPath::Udp, rx_mean(190_000)),
        Readiness::Not(vec!["TX (UDP)".to_string()])
    );
}

#[test]
fn an_rx_mean_under_the_clock_sign_is_the_clock_item() {
    // alex-nb 26.11.2026: 0.02 ms behind its USB NIC's lagging follower
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(20_000)),
        Readiness::Not(vec!["clock (RX mean 0.02 ms)".to_string()])
    );
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(CLOCK_SIGN_MEAN_NS - 1)),
        Readiness::Not(vec!["clock (RX mean 0.05 ms)".to_string()])
    );
    // the sign itself and FOH's 0.18 ms hold
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(CLOCK_SIGN_MEAN_NS)),
        Readiness::Ok
    );
    assert_eq!(
        readiness(Some(0), TxPath::default(), rx_mean(180_000)),
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
        readiness(f.pc_problems, f.tx, f.rx),
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
        readiness(none.pc_problems, none.tx, none.rx),
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
        readiness(Some(0), TxPath::default(), tick_without_packet),
        Readiness::Checking
    );
    assert_eq!(
        readiness(Some(3), TxPath::default(), None),
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
