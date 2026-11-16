use super::*;

fn look(idle: bool, changed: bool, restarts_rose: u64) -> Look {
    Look {
        idle,
        changed,
        restarts_rose,
    }
}

#[test]
fn a_look_logs_the_change_both_ways_once() {
    assert_eq!(
        notes(look(true, true, 0), false, 7),
        vec![Note::Idle { after_pulls: true }]
    );
    assert_eq!(notes(look(false, true, 0), false, 7), vec![Note::Pulls]);
    // No change: nothing, whether the driver pulls or the look drains.
    assert_eq!(notes(look(true, false, 0), false, 7), vec![]);
    assert_eq!(notes(look(false, false, 0), false, 7), vec![]);
}

#[test]
fn the_start_s_idle_before_any_pull_is_no_warning() {
    // The first look comes 250 ms in: a driver (SoundGrid) whose first open
    // takes longer reads idle with no pull yet, which is the start, not a
    // driver that stopped.
    let start = notes(look(true, true, 0), false, 0);
    assert_eq!(start, vec![Note::Idle { after_pulls: false }]);
    assert!(!start[0].warns());
    let stopped = notes(look(true, true, 0), false, 1);
    assert_eq!(stopped, vec![Note::Idle { after_pulls: true }]);
    assert!(stopped[0].warns());
}

#[test]
fn a_look_logs_a_cleared_poison_and_the_restarts_rise() {
    assert_eq!(notes(look(false, false, 0), true, 7), vec![Note::Panicked]);
    assert_eq!(
        notes(look(false, false, 3), false, 7),
        vec![Note::Restarted(3)]
    );
    assert_eq!(
        notes(look(false, false, 1), false, 7),
        vec![Note::Restarted(1)]
    );
    assert_eq!(
        notes(look(true, true, 2), true, 7),
        vec![
            Note::Panicked,
            Note::Idle { after_pulls: true },
            Note::Restarted(2)
        ]
    );
}

#[test]
fn only_the_driver_pulling_and_the_start_s_idle_are_info() {
    assert!(Note::Panicked.warns());
    assert!(Note::Idle { after_pulls: true }.warns());
    assert!(!Note::Idle { after_pulls: false }.warns());
    assert!(Note::Restarted(1).warns());
    assert!(!Note::Pulls.warns());
    let texts = [
        Note::Panicked.text(),
        Note::Idle { after_pulls: true }.text(),
        Note::Pulls.text(),
        Note::Restarted(1).text(),
    ];
    for (i, a) in texts.iter().enumerate() {
        assert!(!a.is_empty());
        for b in &texts[i + 1..] {
            assert_ne!(a, b, "each line its own text");
        }
    }
    assert!(
        Note::Idle { after_pulls: true }
            .text()
            .contains("drains the flow itself")
    );
    assert!(Note::Pulls.text().contains("pulls the to-ASIO audio again"));
    assert!(Note::Restarted(1).text().contains("started again"));
    assert!(Note::Panicked.text().contains("panicked"));
}

#[test]
fn the_look_runs_every_250_ms_as_dstack_asks() {
    // dstack v0.6.155's Consumers line: an `Every` of 250 ms (past a
    // driver buffer, under 150 ms at the default target; inside a flow's
    // 2 s keepalive).
    assert_eq!(KEEP_EVERY, Duration::from_millis(250));
}

/// The bridge instance's glue (Windows only, so read as text here).
const RUN_BRIDGE: &str = include_str!("../run_bridge.rs");

fn at(needle: &str) -> usize {
    let found: Vec<usize> = RUN_BRIDGE.match_indices(needle).map(|(i, _)| i).collect();
    assert_eq!(found.len(), 1, "{needle}: once in run_bridge.rs");
    found[0]
}

/// The body of `fn <name>(` in run_bridge.rs: up to the next `\nfn ` or
/// `\npub fn `.
fn body(name: &str) -> std::ops::Range<usize> {
    let start = at(&format!("\nfn {name}("));
    let rest = &RUN_BRIDGE[start + 1..];
    let end = [rest.find("\nfn "), rest.find("\npub fn ")]
        .into_iter()
        .flatten()
        .min()
        .map_or(RUN_BRIDGE.len(), |e| start + 1 + e);
    start..end
}

#[test]
fn the_bridge_instance_hands_the_to_asio_side_to_the_driver_s_pull() {
    // dstack v0.6.155's consumer port: the rings' claim before the pump,
    // a pump map with no RX channel, the pull handed over before the
    // driver's runner (before its first open).
    let call = at("pull_to_driver(&mut b,");
    let pump = at("Pump::new(");
    let runner = RUN_BRIDGE
        .find("spawn_runner(")
        .expect("the runner's start");
    assert!(call < pump, "the pull is made before Pump::new");
    assert!(
        call < runner,
        "the pull is handed over before the driver's runner"
    );
    let helper = body("pull_to_driver");
    let claim = at(".app_intake()");
    let new_pull = at("RxPull::new(");
    let hand_over = at(".pull_to_asio(");
    for (what, i) in [
        ("claim", claim),
        ("RxPull::new", new_pull),
        ("hand-over", hand_over),
    ] {
        assert!(helper.contains(&i), "{what} inside pull_to_driver");
    }
    assert!(
        claim < new_pull && new_pull < hand_over,
        "claim, pull, hand-over"
    );
    let map = RUN_BRIDGE[..pump]
        .rfind("let map = Map {")
        .expect("the pump's map");
    assert!(
        RUN_BRIDGE[map..pump].contains("rx: vec![],"),
        "the pump's map carries no RX channel"
    );
}

#[test]
fn the_bridge_instance_looks_at_the_pull_on_its_own_timer_only() {
    // Never the pump, never the driver's callback, never the 10 s counts
    // tick: its own 250 ms timer in the run loop.
    let timer = at("Every::new(crate::to_asio_pull::KEEP_EVERY");
    let due = at("if keep.due(now) {");
    let call = at("look_at_pull(&shared,");
    assert!(timer < due && due < call);
    assert!(call - due < 60, "the look runs right on its timer");
    let counts = at("if counts.due(now) {");
    let counts_end = at("\"bridge counts\"");
    assert!(
        !(counts..counts_end).contains(&call),
        "not inside the counts tick"
    );
    let look = body("look_at_pull");
    assert!(
        look.contains(&at(".keep_pulled()")),
        "keep_pulled only in look_at_pull"
    );
    assert!(look.contains(&at("crate::to_asio_pull::notes(")));
}
