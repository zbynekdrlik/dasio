use super::*;

fn flow(packets: u64, peak: u64, last_max: u64, last_min_ns: Option<u64>) -> Flow {
    Flow {
        packets,
        peak,
        last_max,
        last_min_ns,
    }
}

#[test]
fn samples_turn_into_ns_rounded_up() {
    assert_eq!(samples_ns(0, 48_000), 0);
    assert_eq!(samples_ns(48, 48_000), 1_000_000);
    assert_eq!(samples_ns(96, 96_000), 1_000_000);
    // 1 sample at 48 kHz = 20833.33 ns: rounded up.
    assert_eq!(samples_ns(1, 48_000), 20_834);
    // A rate of 0 never divides by zero.
    assert_eq!(samples_ns(1, 0), 1_000_000_000);
    assert_eq!(samples_ns(u64::MAX, 1), u64::MAX);
}

#[test]
fn the_device_figures_come_from_the_slots_that_received() {
    // (packets, peak samples, last second's largest samples, least ns)
    // at 96 kHz.
    let flows = [
        flow(0, 9_999, 9_999, Some(1)),
        flow(1_000, 48, 24, Some(100_000)),
        flow(500, 96, 30, Some(150_000)),
        flow(0, 0, 0, None),
    ];
    assert_eq!(
        measure(flows, 96_000),
        Some(Report {
            peak_ns: 1_000_000,
            last: Some((100_000, 312_500)),
        })
    );
    // An idle slot's figures are no measurement.
    assert_eq!(measure([flow(0, 960, 960, None)], 96_000), None);
    assert_eq!(measure(std::iter::empty(), 96_000), None);
    // A received slot with a 0 age is a measured 0.
    assert_eq!(
        measure([flow(10, 0, 0, Some(0))], 48_000),
        Some(Report {
            peak_ns: 0,
            last: Some((0, 0)),
        })
    );
    // A slot without a packet in the last second: only its peak counts.
    assert_eq!(
        measure(
            [flow(10, 48, 0, None), flow(10, 24, 12, Some(200_000))],
            48_000
        ),
        Some(Report {
            peak_ns: 1_000_000,
            last: Some((200_000, 250_000)),
        })
    );
    assert_eq!(
        measure([flow(10, 48, 0, None)], 48_000),
        Some(Report {
            peak_ns: 1_000_000,
            last: None,
        })
    );
    // The largest is never below the least (its samples round down to the
    // stamp's sample, the least is in ns).
    assert_eq!(
        measure([flow(10, 0, 0, Some(5_000))], 48_000).and_then(|r| r.last),
        Some((5_000, 5_000))
    );
}

#[test]
fn the_shared_figures_keep_the_peak_and_the_latest_second() {
    let a = Arrival::default();
    assert_eq!(a.report(), None);
    a.set(None);
    assert_eq!(a.report(), None);
    let first = Report {
        peak_ns: 700_000,
        last: Some((100_000, 400_000)),
    };
    a.set(Some(first));
    assert_eq!(a.report(), Some(first));
    // A lower peak (a slot dstack started again) never lowers it; the last
    // second is the newest look's, gone when it placed nothing.
    a.set(Some(Report {
        peak_ns: 300_000,
        last: None,
    }));
    assert_eq!(
        a.report(),
        Some(Report {
            peak_ns: 700_000,
            last: None,
        })
    );
    a.set(Some(Report {
        peak_ns: 3_600_000,
        last: Some((0, 3_600_000)),
    }));
    // A clone is the same figures (the status closure's).
    assert_eq!(
        a.clone().report(),
        Some(Report {
            peak_ns: 3_600_000,
            last: Some((0, 3_600_000)),
        })
    );
    a.set(Some(Report {
        peak_ns: u64::MAX,
        last: None,
    }));
    assert_eq!(a.report().map(|r| r.peak_ns), Some(u64::MAX - 1));
}

#[test]
fn the_watch_says_the_figures_when_the_peak_since_start_rises() {
    let mut w = Watch::default();
    let status = w.arrival();
    // No packet: nothing to say.
    assert_eq!(w.observe([flow(0, 0, 0, None)], 48_000), None);
    assert_eq!(w.report(), None);
    // A first measurement is said, even a 0 age.
    let first = w
        .observe([flow(100, 0, 0, Some(0))], 48_000)
        .expect("a first measurement is said");
    assert_eq!(first.peak_ns, 0);
    // 0.5 ms peak (24 samples at 48 kHz): a rise, said.
    let rise = w
        .observe([flow(100, 24, 12, Some(50_000))], 48_000)
        .expect("a rise is said");
    assert_eq!(rise.peak_ns, 500_000);
    assert_eq!(rise.last, Some((50_000, 250_000)));
    // The same peak or a slot started again (peak 0): nothing new.
    assert_eq!(w.observe([flow(200, 24, 6, Some(40_000))], 48_000), None);
    assert_eq!(w.observe([flow(5, 0, 0, None)], 48_000), None);
    // The status closure reads the same figures (peak kept, last second
    // the newest look's).
    assert_eq!(
        status.report(),
        Some(Report {
            peak_ns: 500_000,
            last: None,
        })
    );
}

#[test]
fn the_log_text_is_a_measurement_never_advice() {
    let r = Report {
        peak_ns: 720_000,
        last: Some((100_000, 350_000)),
    };
    assert_eq!(
        r.text(),
        "RX arrival (measured packet age at placement): peak 0.72 ms since start, last second 0.10-0.35 ms"
    );
    assert_eq!(
        Report {
            peak_ns: 980_000,
            last: None
        }
        .text(),
        "RX arrival (measured packet age at placement): peak 0.98 ms since start, no packet in the last second"
    );
    for t in [r.text(), r.suffix(), r.json()] {
        for advice in ["recommend", "latency_ns", "set ", "below"] {
            assert!(!t.contains(advice), "{advice} in {t}");
        }
    }
}

#[test]
fn the_tray_part_shows_the_last_second_and_the_peak() {
    assert_eq!(
        Report {
            peak_ns: 720_000,
            last: Some((100_000, 350_000)),
        }
        .suffix(),
        " · RX arrival 0.10-0.35 ms, peak 0.72 ms"
    );
    assert_eq!(
        Report {
            peak_ns: 720_000,
            last: None,
        }
        .suffix(),
        " · RX arrival peak 0.72 ms"
    );
    assert_eq!(suffix(None), "");
}

#[test]
fn the_json_keys_come_only_with_a_measurement() {
    assert_eq!(json(None), "");
    assert_eq!(
        json(Some(Report {
            peak_ns: 720_000,
            last: Some((100_000, 350_000)),
        })),
        "\"rx_arrival_peak_ns\":720000,\"rx_arrival_last_min_ns\":100000,\"rx_arrival_last_max_ns\":350000,"
    );
    assert_eq!(
        json(Some(Report {
            peak_ns: 720_000,
            last: None,
        })),
        "\"rx_arrival_peak_ns\":720000,"
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
fn the_figures_are_logged_at_info_never_as_a_warning() {
    let text = logged(|| {
        log(&Report {
            peak_ns: 7_000_000,
            last: Some((100_000, 6_900_000)),
        })
    });
    assert!(text.contains(" INFO "), "at INFO: {text}");
    assert!(!text.contains("WARN"), "{text}");
    assert!(text.contains("arrival_peak_ns=7000000"), "{text}");
    assert!(text.contains("last_min_ns=Some(100000)"), "{text}");
    assert!(text.contains("last_max_ns=Some(6900000)"), "{text}");
    assert!(text.contains("peak 7.00 ms since start"), "{text}");
}

#[test]
fn the_flows_are_every_rx_slot_s_figures() {
    use dstack::media::ring::Ring;
    use dstack::media::rx::{MAX_FLOWS, RxShared};
    let rings: std::sync::Arc<[Ring]> = (0..2).map(|_| Ring::new()).collect::<Vec<_>>().into();
    let rx = RxShared::new(48_000, std::net::Ipv4Addr::LOCALHOST, rings).expect("the RX side");
    let all: Vec<Flow> = flows(&rx).collect();
    // Every slot, received or not: an idle one reads all zero with no
    // last second, which `measure` leaves out.
    assert_eq!(all.len(), MAX_FLOWS);
    assert!(all.iter().all(|&f| f == Flow::default()), "{all:?}");
    assert_eq!(measure(all, 48_000), None);
}
