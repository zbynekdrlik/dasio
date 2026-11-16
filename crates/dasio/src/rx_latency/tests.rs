use super::*;

#[test]
fn samples_turn_into_ns_rounded_up() {
    assert_eq!(samples_ns(0, 48_000), 0);
    assert_eq!(samples_ns(48, 48_000), 1_000_000);
    assert_eq!(samples_ns(96, 96_000), 1_000_000);
    // 1 sample at 48 kHz = 20833.33 ns: rounded up, never short.
    assert_eq!(samples_ns(1, 48_000), 20_834);
    // A rate of 0 never divides by zero.
    assert_eq!(samples_ns(1, 0), 1_000_000_000);
}

#[test]
fn the_recommendation_is_the_peak_and_the_margin_rounded_up_to_a_quarter_ms() {
    assert_eq!(MARGIN_NS, 250_000);
    assert_eq!(STEP_NS, 250_000);
    assert_eq!(recommended_ns(0), 250_000);
    assert_eq!(recommended_ns(1), 500_000);
    assert_eq!(recommended_ns(250_000), 500_000);
    assert_eq!(recommended_ns(410_000), 750_000);
    assert_eq!(recommended_ns(500_000), 750_000);
    assert_eq!(recommended_ns(720_000), 1_000_000);
    assert_eq!(recommended_ns(750_001), 1_250_000);
    // dsg 15.11: a 7 ms burst asks for 7.25 ms.
    assert_eq!(recommended_ns(7_000_000), 7_250_000);
    // Never overflows.
    assert_eq!(recommended_ns(u64::MAX), u64::MAX);
}

#[test]
fn the_flow_peak_is_the_largest_of_the_slots_that_received() {
    // (packets, lateness_peak samples) at 96 kHz.
    let flows = [(0, 9_999), (1_000, 48), (500, 96), (0, 0)];
    assert_eq!(peak_ns(flows, 96_000), Some(1_000_000));
    // An idle slot's figure is no measurement.
    assert_eq!(peak_ns([(0, 960), (0, 0)], 96_000), None);
    assert_eq!(peak_ns(std::iter::empty(), 96_000), None);
    // A received slot with a 0 age is a measured 0.
    assert_eq!(peak_ns([(10, 0)], 48_000), Some(0));
}

#[test]
fn the_peak_since_start_only_grows() {
    let p = Peak::default();
    assert_eq!(p.get(), None);
    assert_eq!(p.report(1_000_000), None);
    p.seen(None);
    assert_eq!(p.get(), None);
    p.seen(Some(0));
    assert_eq!(p.get(), Some(0));
    p.seen(Some(3_600_000));
    p.seen(Some(400_000));
    p.seen(None);
    assert_eq!(p.get(), Some(3_600_000));
    p.seen(Some(7_000_000));
    assert_eq!(p.get(), Some(7_000_000));
    // A clone is the same peak (the status closure's).
    assert_eq!(p.clone().get(), Some(7_000_000));
    p.seen(Some(u64::MAX));
    assert_eq!(p.get(), Some(u64::MAX - 1));
}

#[test]
fn the_watch_says_the_report_only_when_the_recommendation_changes() {
    let mut w = Watch::new(1_000_000);
    let status = w.peak();
    // No packet: nothing to say.
    assert_eq!(w.observe([(0, 0)], 48_000), None);
    assert_eq!(w.report(), None);
    // 0.5 ms peak (24 samples at 48 kHz) -> 0.75 ms.
    let first = w
        .observe([(100, 24)], 48_000)
        .expect("a first measurement is said");
    assert_eq!(first, Report::of(1_000_000, 500_000));
    assert_eq!(first.recommended_ns, 750_000);
    // A smaller look and the same step: nothing new.
    assert_eq!(w.observe([(200, 12)], 48_000), None);
    // A dstack slot whose counts started again (peak 0) never lowers it.
    assert_eq!(w.observe([(5, 0)], 48_000), None);
    assert_eq!(w.report().map(|r| r.peak_ns), Some(500_000));
    // A burst past the step: said again, now below the configured 1 ms.
    let burst = w.observe([(400, 48)], 48_000).expect("a new step is said");
    assert_eq!(burst.recommended_ns, 1_250_000);
    assert!(burst.below());
    // The status closure reads the same peak.
    assert_eq!(status.report(1_000_000), Some(burst));
}

#[test]
fn below_is_a_configured_latency_under_the_recommendation() {
    assert!(!Report::of(1_000_000, 500_000).below());
    assert!(!Report::of(1_000_000, 750_000).below());
    assert!(Report::of(1_000_000, 750_001).below());
    assert!(Report::of(5_000_000, 7_000_000).below());
}

#[test]
fn the_log_text_names_the_figures_and_the_step_to_take() {
    assert_eq!(
        Report::of(2_000_000, 720_000).text(),
        "RX latency 2.00 ms holds: measured arrival peak 0.72 ms since start, recommended latency_ns = 1000000 (1.00 ms)"
    );
    assert_eq!(
        Report::of(1_000_000, 980_000).text(),
        "RX latency 1.00 ms is below the recommended 1.25 ms (measured arrival peak 0.98 ms since start): set latency_ns = 1250000 in device.toml at the next restart"
    );
}

#[test]
fn the_tray_part_shows_the_configured_and_the_recommended_latency() {
    assert_eq!(
        Report::of(1_000_000, 410_000).suffix(),
        " · RX 1.00 ms (rec. 0.75)"
    );
    assert_eq!(
        Report::of(1_000_000, 980_000).suffix(),
        " · RX 1.00 ms < rec. 1.25 ms"
    );
    assert_eq!(suffix(None), "");
}

#[test]
fn the_json_keys_come_only_with_a_measurement() {
    assert_eq!(json(None), "");
    assert_eq!(
        json(Some(Report::of(1_000_000, 410_000))),
        "\"rx_latency_ns\":1000000,\"rx_arrival_peak_ns\":410000,\"rx_latency_recommended_ns\":750000,\"rx_latency_below\":false,"
    );
    assert_eq!(
        json(Some(Report::of(1_000_000, 980_000))),
        "\"rx_latency_ns\":1000000,\"rx_arrival_peak_ns\":980000,\"rx_latency_recommended_ns\":1250000,\"rx_latency_below\":true,"
    );
}

/// The log lines `f` writes at INFO and above, without colours.
fn logged(f: impl FnOnce()) -> String {
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = Buf::default();
    let to = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || to.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn a_latency_below_the_recommendation_is_logged_at_warn() {
    let below = logged(|| log(&Report::of(1_000_000, 980_000)));
    assert!(below.contains(" WARN "), "at WARN: {below}");
    assert!(
        below.contains("set latency_ns = 1250000 in device.toml at the next restart"),
        "{below}"
    );
    assert!(below.contains("latency_ns=1000000"), "{below}");
    assert!(below.contains("arrival_peak_ns=980000"), "{below}");
    assert!(below.contains("recommended_ns=1250000"), "{below}");
    let holds = logged(|| log(&Report::of(2_000_000, 720_000)));
    assert!(holds.contains(" INFO "), "at INFO: {holds}");
    assert!(!holds.contains("WARN"), "{holds}");
    assert!(holds.contains("RX latency 2.00 ms holds"), "{holds}");
}

#[test]
fn the_flows_are_every_rx_slot_s_packets_and_peak() {
    use dstack::media::ring::Ring;
    use dstack::media::rx::{MAX_FLOWS, RxShared};
    let rings: std::sync::Arc<[Ring]> = (0..2).map(|_| Ring::new()).collect::<Vec<_>>().into();
    let rx = RxShared::new(48_000, std::net::Ipv4Addr::LOCALHOST, rings).expect("the RX side");
    let all: Vec<(u64, u64)> = flows(&rx).collect();
    // Every slot, received or not: an idle one reads (0, 0), which
    // `peak_ns` leaves out.
    assert_eq!(all.len(), MAX_FLOWS);
    assert!(all.iter().all(|&f| f == (0, 0)), "{all:?}");
    assert_eq!(peak_ns(all, 48_000), None);
}
