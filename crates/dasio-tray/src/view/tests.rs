use super::*;

fn snap(late: u64, beat: u64, attached: bool) -> Snapshot {
    Snapshot {
        rx: 32,
        tx: 32,
        block: 128,
        rate: 96_000,
        latency_in: 224,
        latency_out: 352,
        device_pid: 1,
        seq: 1,
        ack: 1,
        heartbeat_ms: beat,
        attached,
        late,
        missed: 0,
    }
}

fn running(n: u32, name: &str, s: Snapshot) -> Reading {
    Reading {
        instance: n,
        name: name.into(),
        service: Service::Running,
        snap: Some(s),
    }
}

#[test]
fn no_instance_is_grey() {
    let v = Tracker::default().update(0, &[]);
    assert_eq!(v.colour, Colour::Grey);
    assert_eq!(v.tooltip, "dasio: no instance installed");
    assert!(v.lines.is_empty());
}

#[test]
fn a_running_attached_instance_with_nothing_late_is_green() {
    let mut t = Tracker::default();
    let v = t.update(0, &[running(2, "master-asio-dante", snap(852, 10, true))]);
    assert_eq!(v.colour, Colour::Green);
    assert_eq!(v.tooltip, "dasio: 1 running, all OK");
    assert_eq!(
        v.lines,
        vec![
            "master-asio-dante (instance 2): OK · DAW attached · 128 @ 96 kHz · 32/32 ch · in 2.33 / out 3.67 ms · late 0/min (852 total)"
        ]
    );
}

#[test]
fn a_late_block_in_the_last_minute_is_yellow_and_counted() {
    let mut t = Tracker::default();
    t.update(0, &[running(1, "band", snap(10, 1, true))]);
    let v = t.update(30, &[running(1, "band", snap(13, 2, true))]);
    assert_eq!(v.colour, Colour::Yellow);
    assert_eq!(v.tooltip, "dasio: band 3 late");
    assert!(
        v.lines[0].contains("late 3/min (13 total)"),
        "{}",
        v.lines[0]
    );
    // A minute on with no new late block: green again.
    t.update(61, &[running(1, "band", snap(13, 3, true))]);
    let v = t.update(92, &[running(1, "band", snap(13, 4, true))]);
    assert_eq!(v.colour, Colour::Green, "{v:?}");
}

#[test]
fn no_daw_is_yellow() {
    let v = Tracker::default().update(0, &[running(1, "band", snap(0, 1, false))]);
    assert_eq!(v.colour, Colour::Yellow);
    assert_eq!(v.tooltip, "dasio: band no DAW");
    assert!(v.lines[0].contains("no DAW ·"), "{}", v.lines[0]);
}

#[test]
fn a_heartbeat_that_stops_is_red() {
    let mut t = Tracker::default();
    t.update(0, &[running(1, "band", snap(0, 7, true))]);
    let v = t.update(STUCK_S - 1, &[running(1, "band", snap(0, 7, true))]);
    assert_eq!(v.colour, Colour::Green, "not yet");
    let v = t.update(STUCK_S, &[running(1, "band", snap(0, 7, true))]);
    assert_eq!(v.colour, Colour::Red);
    assert_eq!(v.tooltip, "dasio: band stuck");
    // It moves again: fine.
    let v = t.update(STUCK_S + 1, &[running(1, "band", snap(0, 8, true))]);
    assert_eq!(v.colour, Colour::Green);
}

#[test]
fn a_stopped_service_is_red_and_the_worst_one_names_the_tooltip() {
    let mut t = Tracker::default();
    let stopped = Reading {
        instance: 1,
        name: "band".into(),
        service: Service::Stopped,
        snap: None,
    };
    let v = t.update(0, &[stopped, running(2, "master", snap(0, 1, false))]);
    assert_eq!(v.colour, Colour::Red);
    assert_eq!(
        v.tooltip, "dasio: band stopped",
        "the yellow one is not named"
    );
    assert_eq!(v.lines[0], "band (instance 1): stopped");
    let pending = Reading {
        instance: 3,
        name: "x".into(),
        service: Service::Other("START_PENDING".into()),
        snap: None,
    };
    let v = t.update(1, &[pending]);
    assert_eq!(v.colour, Colour::Red);
    assert_eq!(v.lines, vec!["x (instance 3): start_pending"]);
}

#[test]
fn a_running_service_without_its_mapping_is_red() {
    let r = Reading {
        instance: 1,
        name: "band".into(),
        service: Service::Running,
        snap: None,
    };
    let v = Tracker::default().update(0, &[r]);
    assert_eq!(v.colour, Colour::Red);
    assert_eq!(v.tooltip, "dasio: band no device");
}

#[test]
fn a_restarted_device_starts_its_late_count_again() {
    let mut t = Tracker::default();
    t.update(0, &[running(1, "band", snap(900, 1, true))]);
    let v = t.update(10, &[running(1, "band", snap(2, 2, true))]);
    assert_eq!(
        v.colour,
        Colour::Green,
        "no underflow, no false late: {v:?}"
    );
    let v = t.update(20, &[running(1, "band", snap(4, 3, true))]);
    assert!(v.lines[0].contains("late 2/min"), "{}", v.lines[0]);
}

#[test]
fn a_long_tooltip_is_cut_to_the_windows_limit() {
    let many: Vec<Reading> = (1..=16)
        .map(|n| Reading {
            instance: n,
            name: format!("a-long-device-name-{n}"),
            service: Service::Stopped,
            snap: None,
        })
        .collect();
    let v = Tracker::default().update(0, &many);
    assert_eq!(v.tooltip.chars().count(), TOOLTIP_MAX);
    assert!(v.tooltip.ends_with('…'));
    assert_eq!(cut("short".into(), TOOLTIP_MAX), "short");
}

#[test]
fn names_come_from_the_asio_entry_with_the_instance_s_clsid() {
    let entries = vec![
        ("band-asio-dante".to_string(), "{AAAA-1}".to_string()),
        ("master-asio-dante".to_string(), "{aaaa-2}".to_string()),
    ];
    assert_eq!(name_of(2, "{AAAA-2}", &entries), "master-asio-dante");
    assert_eq!(name_of(3, "{AAAA-3}", &entries), "dasio 3");
}

#[test]
fn only_dasio_n_services_are_instances() {
    assert_eq!(instance_of_service("dasio-1"), Some(1));
    assert_eq!(instance_of_service("dasio-16"), Some(16));
    assert_eq!(instance_of_service("dasio-0"), None);
    assert_eq!(instance_of_service("dasio-17"), None);
    assert_eq!(instance_of_service("dasio-tray"), None);
    assert_eq!(instance_of_service("dante-asio-1"), None);
}

#[test]
fn a_reading_exactly_a_window_old_still_counts() {
    let mut t = Tracker::default();
    t.update(0, &[running(1, "band", snap(10, 1, true))]);
    let v = t.update(LATE_WINDOW_S, &[running(1, "band", snap(13, 2, true))]);
    assert!(v.lines[0].contains("late 3/min"), "{}", v.lines[0]);
    let v = t.update(LATE_WINDOW_S + 1, &[running(1, "band", snap(13, 3, true))]);
    assert!(v.lines[0].contains("late 0/min"), "{}", v.lines[0]);
}
