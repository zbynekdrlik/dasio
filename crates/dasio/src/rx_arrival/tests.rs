use super::*;

fn flow(packets: u64, peak: u64, last_max: u64, last_min_ns: Option<u64>) -> Flow {
    Flow {
        packets,
        peak,
        last_max,
        last_min_ns,
        ..Flow::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        })
    );
    assert_eq!(
        measure([flow(10, 48, 0, None)], 48_000),
        Some(Report {
            peak_ns: 1_000_000,
            last: None,
            ..Default::default()
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
        ..Default::default()
    };
    a.set(Some(first));
    assert_eq!(a.report(), Some(first));
    // A lower peak (a slot dstack started again) never lowers it; the last
    // second is the newest look's, gone when it placed nothing.
    a.set(Some(Report {
        peak_ns: 300_000,
        last: None,
        ..Default::default()
    }));
    assert_eq!(
        a.report(),
        Some(Report {
            peak_ns: 700_000,
            last: None,
            ..Default::default()
        })
    );
    a.set(Some(Report {
        peak_ns: 3_600_000,
        last: Some((0, 3_600_000)),
        ..Default::default()
    }));
    // A clone is the same figures (the status closure's).
    assert_eq!(
        a.clone().report(),
        Some(Report {
            peak_ns: 3_600_000,
            last: Some((0, 3_600_000)),
            ..Default::default()
        })
    );
    a.set(Some(Report {
        peak_ns: u64::MAX,
        last: None,
        ..Default::default()
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
            ..Default::default()
        })
    );
}

#[test]
fn the_log_text_is_a_measurement_never_advice() {
    let r = Report {
        peak_ns: 720_000,
        last: Some((100_000, 350_000)),
        ..Default::default()
    };
    assert_eq!(
        r.text(),
        "RX arrival (measured packet age at arrival): peak 0.72 ms since start, last second at the 10 s look 0.10-0.35 ms"
    );
    assert_eq!(
        Report {
            peak_ns: 980_000,
            last: None,
            ..Default::default()
        }
        .text(),
        "RX arrival (measured packet age at arrival): peak 0.98 ms since start, no packet in the last second at the 10 s look"
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
            ..Default::default()
        }
        .suffix(),
        " · RX arrival 0.10-0.35 ms, peak 0.72 ms"
    );
    assert_eq!(
        Report {
            peak_ns: 720_000,
            last: None,
            ..Default::default()
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
            ..Default::default()
        })),
        "\"rx_arrival_peak_ns\":720000,\"rx_arrival_last_min_ns\":100000,\"rx_arrival_last_max_ns\":350000,"
    );
    assert_eq!(
        json(Some(Report {
            peak_ns: 720_000,
            last: None,
            ..Default::default()
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
            ..Default::default()
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

fn totals(sum_ns: u64, count: u64) -> Totals {
    Totals { sum_ns, count }
}

/// A received slot with running totals of its ages and read waits.
fn running(age: Totals, wait: Totals, wait_peak_us: u64) -> Flow {
    Flow {
        packets: age.count.max(1),
        age,
        wait,
        wait_peak_us,
        ..Flow::default()
    }
}

#[test]
fn two_looks_at_the_totals_give_the_packets_between_them() {
    assert_eq!(
        totals(5_000, 30).since(totals(2_000, 10)),
        totals(3_000, 20)
    );
    // The same count: no packet between the looks, never the whole total.
    assert_eq!(totals(5_000, 10).since(totals(5_000, 10)), totals(0, 0));
    // A look between dstack's sum and count writes: the same count with a
    // larger sum is still no packet (never a sum without its packet).
    assert_eq!(totals(5_400, 10).since(totals(5_000, 10)), totals(0, 0));
    // The sum wrapped (modulo 2^64, dstack's rule): the diff is still right.
    assert_eq!(
        totals(100, 12).since(totals(u64::MAX - 99, 10)),
        totals(200, 2)
    );
    // The count went down: the slot's counts started again, so all of the
    // new look is new.
    assert_eq!(totals(700, 3).since(totals(9_000, 50)), totals(700, 3));
    // From nothing: everything.
    assert_eq!(totals(700, 3).since(Totals::default()), totals(700, 3));
}

#[test]
fn the_mean_weighs_every_packet_and_needs_one() {
    assert_eq!(mean(Vec::<Totals>::new()), None);
    assert_eq!(mean([totals(0, 0), totals(0, 0)]), None);
    // One busy slot and one with a single packet: per packet, (90 000 +
    // 1 000) / 901 = 100.99, not per slot ((100 + 1 000) / 2 = 550).
    assert_eq!(mean([totals(900 * 100, 900), totals(1_000, 1)]), Some(100));
    // Rounded down.
    assert_eq!(mean([totals(10, 3)]), Some(3));
    // Sums past u64 together never overflow.
    assert_eq!(
        mean([totals(u64::MAX, 2), totals(u64::MAX, 2)]),
        Some(u64::MAX / 2)
    );
    assert_eq!(mean([totals(0, 5)]), Some(0));
}

#[test]
fn the_device_mean_is_the_tick_s_from_two_looks_never_from_maxima() {
    let earlier = [
        running(totals(1_000_000, 10), totals(500_000, 10), 300),
        Flow::default(),
        running(totals(4_000_000, 20), totals(0, 20), 100),
    ];
    let now = [
        // 30 new packets of 0.2 ms, waits 1 ms each.
        running(totals(7_000_000, 40), totals(30_500_000, 40), 1_200),
        // An idle slot: no measurement, whatever its totals.
        Flow {
            packets: 0,
            age: totals(9_999_999_999, 1),
            ..Flow::default()
        },
        // 10 new packets of 0.3 ms, no new wait.
        running(totals(7_000_000, 30), totals(0, 30), 100),
    ];
    let r = measure_since(now, &earlier, 48_000).expect("measured");
    // (6 ms + 3 ms) / 40 packets.
    assert_eq!(r.mean_ns, Some(225_000));
    assert_eq!(
        r.wait,
        Some(Wait {
            mean_ns: Some(750_000),
            peak_us: 1_200,
        })
    );
    // The peak is not the mean's source: a high peak leaves it alone.
    let mut high = now;
    high[0].peak = 48_000;
    let h = measure_since(high, &earlier, 48_000).expect("measured");
    assert_eq!(h.peak_ns, 1_000_000_000);
    assert_eq!(h.mean_ns, Some(225_000));
    // No packet since the earlier look: no mean, the peaks stay.
    let still = measure_since(earlier, &earlier, 48_000).expect("measured");
    assert_eq!(still.mean_ns, None);
    assert_eq!(
        still.wait,
        Some(Wait {
            mean_ns: None,
            peak_us: 300,
        })
    );
    // A slot that started again counts its new totals.
    let restarted = [running(totals(600_000, 2), totals(0, 2), 0)];
    assert_eq!(
        measure_since(restarted, &earlier, 48_000).and_then(|r| r.mean_ns),
        Some(300_000)
    );
    // A slot the earlier look lacks counts from zero.
    let more = [
        Flow::default(),
        Flow::default(),
        Flow::default(),
        running(totals(400_000, 4), totals(0, 0), 0),
    ];
    assert_eq!(
        measure_since(more, &earlier, 48_000).and_then(|r| r.mean_ns),
        Some(100_000)
    );
    // A slot with no read wait reported: no wait.
    assert_eq!(
        measure([running(totals(10, 1), Totals::default(), 0)], 48_000).and_then(|r| r.wait),
        None
    );
    // Counted waits under 1 µs (a peak of 0 µs) are a wait measurement.
    assert_eq!(
        measure([running(totals(10, 1), totals(500, 1), 0)], 48_000).and_then(|r| r.wait),
        Some(Wait {
            mean_ns: Some(500),
            peak_us: 0,
        })
    );
    // A wait peak alone (no counted wait) is still a wait measurement.
    assert_eq!(
        measure([running(totals(10, 1), Totals::default(), 5)], 48_000).and_then(|r| r.wait),
        Some(Wait {
            mean_ns: None,
            peak_us: 5,
        })
    );
}

#[test]
fn the_watch_keeps_its_own_window_and_the_status_the_latest_tick() {
    let mut w = Watch::default();
    let status = w.arrival();
    // The first look: since the device started.
    w.observe(
        [running(
            totals(2_000_000, 10),
            totals(10_000_000, 10),
            2_000,
        )],
        48_000,
    );
    assert_eq!(status.report().and_then(|r| r.mean_ns), Some(200_000));
    // The second: only the packets since the first.
    w.observe(
        [running(
            totals(5_000_000, 20),
            totals(15_000_000, 20),
            1_500,
        )],
        48_000,
    );
    let r = status.report().expect("measured");
    assert_eq!(r.mean_ns, Some(300_000));
    // The wait's mean is the tick's, its peak the largest seen (a slot
    // that started again never lowers it).
    assert_eq!(
        r.wait,
        Some(Wait {
            mean_ns: Some(500_000),
            peak_us: 2_000,
        })
    );
    // A tick without a packet: no mean, the wait's peak stays.
    w.observe(
        [running(
            totals(5_000_000, 20),
            totals(15_000_000, 20),
            1_500,
        )],
        48_000,
    );
    let r = status.report().expect("measured");
    assert_eq!(r.mean_ns, None);
    assert_eq!(
        r.wait,
        Some(Wait {
            mean_ns: None,
            peak_us: 2_000,
        })
    );
    // A look whose slots report no wait keeps the wait's peak.
    w.observe(
        [running(totals(6_000_000, 21), Totals::default(), 0)],
        48_000,
    );
    let r = status.report().expect("measured");
    assert_eq!(r.mean_ns, Some(1_000_000));
    assert_eq!(
        r.wait,
        Some(Wait {
            mean_ns: None,
            peak_us: 2_000,
        })
    );
    // A higher wait peak rises.
    w.observe(
        [running(totals(6_000_000, 22), totals(4_000, 1), 2_500)],
        48_000,
    );
    assert_eq!(
        status.report().and_then(|r| r.wait),
        Some(Wait {
            mean_ns: Some(4_000),
            peak_us: 2_500,
        })
    );
}

#[test]
fn the_tray_part_shows_the_mean_between_the_last_second_and_the_peak() {
    let r = Report {
        peak_ns: 720_000,
        last: Some((100_000, 350_000)),
        mean_ns: Some(210_000),
        wait: Some(Wait {
            mean_ns: Some(1_000_000),
            peak_us: 2_000,
        }),
    };
    assert_eq!(
        r.suffix(),
        " · RX arrival 0.10-0.35 ms, mean 0.21 ms, peak 0.72 ms"
    );
    assert_eq!(
        Report { last: None, ..r }.suffix(),
        " · RX arrival mean 0.21 ms, peak 0.72 ms"
    );
    assert_eq!(
        json(Some(r)),
        "\"rx_arrival_peak_ns\":720000,\"rx_arrival_last_min_ns\":100000,\"rx_arrival_last_max_ns\":350000,\"rx_arrival_mean_ns\":210000,"
    );
    assert_eq!(
        Report { last: None, ..r }.json(),
        "\"rx_arrival_peak_ns\":720000,\"rx_arrival_mean_ns\":210000,"
    );
    assert_eq!(
        r.text(),
        "RX arrival (measured packet age at arrival): peak 0.72 ms since start, last second at the 10 s look 0.10-0.35 ms, mean 0.21 ms over the last tick; read wait (arrival to the writer's read) mean 1.00 ms, peak 2.00 ms since start"
    );
    let no_wait_mean = Report {
        mean_ns: None,
        wait: Some(Wait {
            mean_ns: None,
            peak_us: 2_000,
        }),
        ..r
    };
    assert_eq!(
        no_wait_mean.text(),
        "RX arrival (measured packet age at arrival): peak 0.72 ms since start, last second at the 10 s look 0.10-0.35 ms; read wait (arrival to the writer's read) peak 2.00 ms since start"
    );
    for t in [r.text(), r.suffix(), r.json(), r.pull_wait_json()] {
        for advice in ["recommend", "latency_ns", "set ", "below"] {
            assert!(!t.contains(advice), "{advice} in {t}");
        }
    }
}

#[test]
fn the_asio_pull_wait_keys_come_only_with_a_measured_wait() {
    assert_eq!(pull_wait_json(None), "");
    let r = Report {
        peak_ns: 720_000,
        ..Report::default()
    };
    assert_eq!(pull_wait_json(Some(r)), "");
    assert_eq!(
        pull_wait_json(Some(Report {
            wait: Some(Wait {
                mean_ns: Some(1_250_000),
                peak_us: 2_670,
            }),
            ..r
        })),
        "\"rx_asio_pull_wait_mean_ns\":1250000,\"rx_asio_pull_wait_peak_us\":2670,"
    );
    assert_eq!(
        pull_wait_json(Some(Report {
            wait: Some(Wait {
                mean_ns: None,
                peak_us: 2_670,
            }),
            ..r
        })),
        "\"rx_asio_pull_wait_peak_us\":2670,"
    );
}

#[test]
fn the_mean_and_the_wait_are_logged() {
    let text = logged(|| {
        log(&Report {
            peak_ns: 7_000_000,
            last: None,
            mean_ns: Some(210_000),
            wait: Some(Wait {
                mean_ns: Some(1_000_000),
                peak_us: 2_000,
            }),
        })
    });
    assert!(text.contains(" INFO "), "{text}");
    assert!(text.contains("arrival_mean_ns=Some(210000)"), "{text}");
    assert!(text.contains("read_wait_mean_ns=Some(1000000)"), "{text}");
    assert!(text.contains("read_wait_peak_us=Some(2000)"), "{text}");
}
