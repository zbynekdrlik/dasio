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
    assert_eq!(notes(look(true, true, 0), false), vec![Note::Idle]);
    assert_eq!(notes(look(false, true, 0), false), vec![Note::Pulls]);
    // No change: nothing, whether the driver pulls or the look drains.
    assert_eq!(notes(look(true, false, 0), false), vec![]);
    assert_eq!(notes(look(false, false, 0), false), vec![]);
}

#[test]
fn a_look_logs_a_cleared_poison_and_the_restarts_rise() {
    assert_eq!(notes(look(false, false, 0), true), vec![Note::Panicked]);
    assert_eq!(
        notes(look(false, false, 3), false),
        vec![Note::Restarted(3)]
    );
    assert_eq!(
        notes(look(false, false, 1), false),
        vec![Note::Restarted(1)]
    );
    assert_eq!(
        notes(look(true, true, 2), true),
        vec![Note::Panicked, Note::Idle, Note::Restarted(2)]
    );
}

#[test]
fn only_the_driver_pulling_again_is_info() {
    assert!(Note::Panicked.warns());
    assert!(Note::Idle.warns());
    assert!(Note::Restarted(1).warns());
    assert!(!Note::Pulls.warns());
    let texts = [
        Note::Panicked.text(),
        Note::Idle.text(),
        Note::Pulls.text(),
        Note::Restarted(1).text(),
    ];
    for (i, a) in texts.iter().enumerate() {
        assert!(!a.is_empty());
        for b in &texts[i + 1..] {
            assert_ne!(a, b, "each line its own text");
        }
    }
    assert!(Note::Idle.text().contains("drains the flow itself"));
    assert!(Note::Pulls.text().contains("pulls the to-ASIO audio again"));
    assert!(Note::Restarted(1).text().contains("started again"));
    assert!(Note::Panicked.text().contains("panicked"));
}

#[test]
fn the_look_runs_past_a_driver_buffer_and_inside_a_flow_s_keepalive() {
    // dstack v0.6.155: a period under the driver's buffer period reads a
    // running driver as stopped; the run loop passes every 100 ms, so the
    // look's own timer fires at most every 300 ms, still far inside 2 s.
    assert!(KEEP_EVERY > MAX_DEFAULT_BUFFER);
    assert!(KEEP_EVERY + Duration::from_millis(100) < FLOW_KEEPALIVE / 4);
    assert_eq!(KEEP_EVERY, Duration::from_millis(250));
}

/// The bridge instance's glue (Windows only, so read as text here).
const RUN_BRIDGE: &str = include_str!("../run_bridge.rs");

fn at(needle: &str) -> usize {
    let found: Vec<usize> = RUN_BRIDGE.match_indices(needle).map(|(i, _)| i).collect();
    assert_eq!(found.len(), 1, "{needle}: once in run_bridge.rs");
    found[0]
}

#[test]
fn the_bridge_instance_hands_the_to_asio_side_to_the_driver_s_pull() {
    // dstack v0.6.155's consumer port: the rings' claim before the pump,
    // a pump map with no RX channel, the pull handed over before the
    // driver's runner (before its first open).
    let claim = at(".app_intake()");
    let new_pull = at("RxPull::new(");
    let hand_over = at(".pull_to_asio(");
    let pump = at("Pump::new(");
    let runner = RUN_BRIDGE
        .find("spawn_runner(")
        .expect("the runner's start");
    assert!(
        claim < new_pull && new_pull < hand_over,
        "claim, pull, hand-over"
    );
    assert!(claim < pump, "the rings are claimed before Pump::new");
    assert!(
        hand_over < runner,
        "the pull is handed over before the driver's runner"
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
    let due = at("keep.due(now)");
    let call = at(".keep_pulled()");
    let make = at("let make: MakeProcess");
    assert!(timer < due && due < call && call < make);
    assert!(call - due < 120, "the look runs right on its timer");
    let counts = at("if counts.due(now) {");
    let counts_end = at("\"bridge counts\"");
    assert!(
        !(counts..counts_end).contains(&call),
        "not inside the counts tick"
    );
    assert!(RUN_BRIDGE.contains("crate::to_asio_pull::notes("));
}
