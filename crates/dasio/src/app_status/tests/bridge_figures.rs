//! dstack v0.6.173 / v0.6.174's bridge figures (dasio#120): the driver
//! callbacks' lateness bins, each direction's converter delay and the
//! underrun tick, in the bridge's status and its run loop; v0.6.176's
//! network read of them (dstack issue 391, dcontroller#108).

use super::*;

#[test]
fn the_bridge_json_carries_every_lateness_bin_by_its_key() {
    // dstack v0.6.173 (dasio#120): the eight bins under dstack's keys, in
    // order, 0 included (a count), before the PC profile's key.
    let j = bridge_json(&BridgeFacts {
        late_bins: [1, 2, 3, 4, 5, 6, 7, 80],
        ..bridge_running()
    });
    assert!(
        j.contains(concat!(
            "\"resets\":1,\"late_250us\":1,\"late_500us\":2,\"late_750us\":3,",
            "\"late_1000us\":4,\"late_1250us\":5,\"late_1500us\":6,\"late_1750us\":7,",
            "\"late_2000us\":80,\"pc_profile_problems\":0,"
        )),
        "{j}"
    );
    let zero = bridge_json(&bridge_running());
    assert!(
        zero.contains(concat!(
            "\"resets\":1,\"late_250us\":0,\"late_500us\":0,\"late_750us\":0,",
            "\"late_1000us\":0,\"late_1250us\":0,\"late_1500us\":0,\"late_1750us\":0,",
            "\"late_2000us\":0,\"pc_profile_problems\":0,"
        )),
        "{zero}"
    );
}

#[test]
fn the_bridge_json_carries_each_direction_s_converter_delay_while_its_player_exists() {
    // dstack v0.6.174 (dasio#120): us, rounded; dsg's 48 taps and 32 taps
    // at 96 <-> 96 kHz, and the 256-tap default.
    for (to, from, keys) in [
        (
            250_000,
            166_667,
            "\"to_asio_converter_delay_us\":250,\"from_asio_converter_delay_us\":167,",
        ),
        (
            1_333_333,
            166_499,
            "\"to_asio_converter_delay_us\":1333,\"from_asio_converter_delay_us\":166,",
        ),
        // Before the driver's first open only the idle from-ASIO player
        // exists: the to-ASIO key is left out, never a 0.
        (0, 250_000, "\"from_asio_converter_delay_us\":250,"),
        (250_000, 0, "\"to_asio_converter_delay_us\":250,"),
        (0, 0, ""),
    ] {
        let j = bridge_json(&BridgeFacts {
            to_asio_converter_delay_ns: to,
            from_asio_converter_delay_ns: from,
            ..bridge_running()
        });
        assert!(
            j.contains(&format!(
                "\"late_2000us\":0,{keys}\"pc_profile_problems\":0,"
            )),
            "{to} / {from}: {j}"
        );
    }
}

#[test]
fn a_bridge_json_with_every_figure_is_one_object_within_dstack_s_cap() {
    // Every optional part set: still one JSON object dstack takes (its
    // `APP_MAX`), so the device's status never drops the app's part.
    use crate::rx_arrival::{Report, Wait};
    let full = BridgeFacts {
        driver: "MiniFuse ASIO Driver".into(),
        reason: Some("the driver gives no clock".into()),
        reason_code: Some("no_clock"),
        retry_in_s: Some(9.6),
        late_bins: [u64::MAX; 8],
        to_asio_converter_delay_ns: 1_333_333,
        from_asio_converter_delay_ns: 1_333_333,
        to_asio_underruns: u64::MAX,
        from_asio_underruns: u64::MAX,
        resets: u64::MAX,
        pc_problems: Some(u32::MAX),
        tx: TxPath::Kernel {
            no_slot: u64::MAX,
            failed: u64::MAX,
        },
        from_asio_age: Some(AGE_DSG),
        rx: Some(Report {
            peak_ns: u64::MAX,
            last: Some((u64::MAX, u64::MAX)),
            mean_ns: Some(u64::MAX),
            wait: Some(Wait {
                mean_ns: Some(u64::MAX),
                peak_us: u64::MAX,
            }),
        }),
        from_asio_pull: Some(pull(u64::MAX, u64::MAX, u64::MAX, u64::MAX)),
        driver_input_latency_frames: u32::MAX,
        driver_output_latency_frames: u32::MAX,
        to_asio_target_ns: u64::MAX,
        from_asio_target_ns: u64::MAX,
        startup: Part {
            in_window: false,
            startup: [u64::MAX; 3],
        },
        ..bridge_running()
    };
    let j = bridge_json(&full);
    assert!(
        dstack::ctl::app_object(&j).is_some(),
        "{} bytes: {j}",
        j.len()
    );
    for key in [
        "\"late_2000us\":18446744073709551615",
        "\"to_asio_converter_delay_us\":1333",
        "\"from_asio_converter_delay_us\":1333",
        // u32::MAX frames at 96 kHz + the block age + the converter.
        "\"bridge_asio_to_dante_us\":44739245639,",
        // The read wait's u64::MAX ns in us + the converter + the frames.
        "\"bridge_dante_to_asio_us\":18446788812953540,",
    ] {
        assert!(j.contains(key), "{key} missing in {j}");
    }
}

/// The runner's status at dsg's MiniFuse (96 kHz, 128 frames) with
/// `late_bins`.
fn minifuse(late_bins: [u64; 8], late_callbacks: u64) -> dstack_asio::Status {
    dstack_asio::Status {
        state: dstack_asio::RunState::Running,
        rate: 96_000,
        buffer_frames: 128,
        late_bins,
        late_callbacks,
        ..Default::default()
    }
}

#[test]
fn the_underrun_tick_names_a_rise_and_the_callbacks_lateness_meanwhile() {
    // dstack v0.6.173's line (dasio#120): only when a count rose since the
    // last tick, with whose it reads as.
    let mut watch = UnderrunWatch::new();
    let quiet = minifuse([0; 8], 0);
    assert_eq!(underrun_tick(&mut watch, &quiet, 0, 0, 1.333333), None);
    // A from-ASIO underrun with a callback 1.0-1.25 ms late at a 1.333 ms
    // target: the target's since dstack v0.6.192 (dstack issue 393): the
    // side's floor, half the 128-frame period (0.667 ms), plus 1.0 ms
    // reaches the target, while 1.0 ms alone does not.
    let late = minifuse([0, 0, 0, 1, 0, 0, 0, 0], 0);
    assert_eq!(
        underrun_tick(&mut watch, &late, 0, 2, 1.333333).as_deref(),
        Some(
            "asio bridge underruns since the last check: from-ASIO 2 (target 1333 us), \
             cause: the target under the side's floor plus the callbacks' lateness; \
             driver callbacks late meanwhile: 1000-1250 us: 1"
        )
    );
    // Nothing rose since: no line.
    assert_eq!(underrun_tick(&mut watch, &late, 0, 2, 1.333333), None);
    // A to-ASIO underrun with no callback late: the bridge's.
    assert_eq!(
        underrun_tick(&mut watch, &late, 1, 2, 1.333333).as_deref(),
        Some(
            "asio bridge underruns since the last check: to-ASIO 1, \
             cause: the bridge (no callback late enough); \
             driver callbacks late meanwhile: none 250 us late or more"
        )
    );
    // Each count is the one the tick was handed: to-ASIO and from-ASIO are
    // not swapped, and the target is the from-ASIO one handed in.
    assert_eq!(
        underrun_tick(&mut watch, &late, 1, 3, 2.0).as_deref(),
        Some(
            "asio bridge underruns since the last check: from-ASIO 1 (target 2000 us), \
             cause: the bridge (no callback late enough); \
             driver callbacks late meanwhile: none 250 us late or more"
        )
    );
    // A callback over two periods late (the runner's `late_callbacks`):
    // a to-ASIO underrun is the driver's then.
    let gap = minifuse([0, 0, 0, 1, 0, 0, 0, 0], 1);
    assert_eq!(
        underrun_tick(&mut watch, &gap, 2, 3, 1.0).as_deref(),
        Some(
            "asio bridge underruns since the last check: to-ASIO 1, \
             cause: driver callback lateness; driver callbacks late meanwhile: \
             over two periods: 1"
        )
    );
}

#[test]
fn the_bridge_s_status_loop_checks_the_underruns_once_a_counts_tick() {
    // dstack v0.6.173: the underrun look runs in the status loop's counts
    // tick, never in the pump or the driver's callback (it formats and
    // logs), on one watch made once before the loop.
    // A Windows checkout has CRLF lines: the needle below spans lines.
    let src = include_str!("../../run_bridge.rs").replace("\r\n", "\n");
    let calls: Vec<usize> = src
        .match_indices("underrun_tick(")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(calls.len(), 1, "one call in run_bridge.rs");
    let tick = src.find("if counts.due(now) {").expect("the counts tick");
    let tick_end = src[tick..].find("\"bridge counts\"").expect("its log line") + tick;
    assert!(
        (tick..tick_end).contains(&calls[0]),
        "the call is not inside the counts tick"
    );
    let watches: Vec<usize> = src
        .match_indices("UnderrunWatch::new()")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(watches.len(), 1, "one watch in run_bridge.rs");
    let looping = src.find("let why = loop {").expect("the run loop");
    assert!(watches[0] < looping, "the watch is made before the loop");
    let callback = src
        .find("let make: MakeProcess")
        .expect("the driver's side");
    assert!(
        calls[0] < callback,
        "the call is in run(), not the driver's side"
    );
    // The counts line carries the lateness bins.
    let bins = src
        .find("late_bins = ?s.late_bins,")
        .expect("the bins logged");
    assert!(
        (tick..tick_end).contains(&bins),
        "the bins in the counts line"
    );
    // Each direction's own converter delay reaches the status (unguarded
    // otherwise: `..BridgeFacts::of` would fill in 0 and drop the keys).
    for side in ["to", "from"] {
        assert_eq!(
            src.matches(&format!(
                "{side}_asio_converter_delay_ns: {side}_stats.converter_delay_ns(),"
            ))
            .count(),
            1,
            "{side}-ASIO converter delay into the status"
        );
        let logged = src
            .find(&format!(
                "{side}_asio_converter_delay_ms = {side}_stats.converter_delay_ns() as f64 / 1e6,"
            ))
            .expect("the delay logged");
        assert!(
            (tick..tick_end).contains(&logged),
            "{side}-ASIO converter delay in the counts line"
        );
    }
    // Each direction's own underruns and the from-ASIO player's target.
    assert!(
        src[calls[0]..tick_end].starts_with(
            "underrun_tick(\n                &mut underruns,\n                &s,\n                to_stats.underruns(),\n                from_stats.underruns(),\n                from_stats.target_ms(),\n            );"
        ),
        "the tick's arguments"
    );
}

#[test]
fn the_underrun_tick_s_counts_and_causes_are_what_dcontroller_reads() {
    // dstack v0.6.176 (dstack issue 391, dcontroller#108): after a tick the
    // watch hands the device's figure slot the counts the tick was given
    // and each side's latest cause; the slot answers dcontroller's read.
    // dstack owns the cause rules (its own tests); this checks that
    // dasio's tick feeds the watch the slot reads, each count on its side.
    use dstack::info::{FigureSlot, OwnFigures, UnderrunCause};
    let slot = FigureSlot::started_at(0);
    slot.set_source(OwnFigures::default);
    let mut watch = UnderrunWatch::new();
    let read = |watch: &UnderrunWatch| {
        watch.publish(&slot);
        let f = slot.figures().expect("a source is set");
        (
            f.to_asio_underruns,
            f.from_asio_underruns,
            f.to_asio_cause,
            f.from_asio_cause,
        )
    };
    assert_eq!(
        read(&watch),
        (0, 0, UnderrunCause::None, UnderrunCause::None)
    );
    // A from-ASIO underrun with a callback 1.0-1.25 ms late at a 1.333 ms
    // target: the driver's; no to-ASIO one yet.
    let late = minifuse([0, 0, 0, 1, 0, 0, 0, 0], 0);
    assert!(underrun_tick(&mut watch, &late, 0, 2, 1.333333).is_some());
    assert_eq!(
        read(&watch),
        (0, 2, UnderrunCause::None, UnderrunCause::Driver)
    );
    // A to-ASIO underrun with no callback late since: the bridge's; the
    // from-ASIO side keeps its latest cause.
    assert!(underrun_tick(&mut watch, &late, 1, 2, 1.333333).is_some());
    assert_eq!(
        read(&watch),
        (1, 2, UnderrunCause::Bridge, UnderrunCause::Driver)
    );
    // A quiet tick keeps the counts and the causes.
    assert_eq!(underrun_tick(&mut watch, &late, 1, 2, 1.333333), None);
    assert_eq!(
        read(&watch),
        (1, 2, UnderrunCause::Bridge, UnderrunCause::Driver)
    );
}

#[test]
fn the_bridge_publishes_its_figures_once_and_its_underruns_every_counts_tick() {
    // dstack v0.6.176's consumer port (dstack issue 391, dcontroller#108):
    // without both calls the device answers nothing to dcontroller's read.
    // A Windows checkout has CRLF lines: the needles below span lines.
    let src = include_str!("../../run_bridge.rs").replace("\r\n", "\n");
    let once = |needle: &str| {
        let found: Vec<usize> = src.match_indices(needle).map(|(i, _)| i).collect();
        assert_eq!(found.len(), 1, "{needle}: once in run_bridge.rs");
        found[0]
    };
    // Once, when the device, the bridge and the runner exist, before the
    // loop: the runner's own status, read live at each request.
    let figures =
        once("lock(&shared).publish_figures(media.bridge_figures(), Arc::clone(&status));");
    once("publish_figures(");
    let device = once("prepared\n        .start(");
    let runner = once("let (runner, status) = match spawn_runner(");
    let looping = once("let why = loop {");
    assert!(device < figures, "after the device's start");
    assert!(runner < figures, "after the runner's start");
    assert!(
        src[runner..figures].contains("return Err(e);\n        }\n    };\n"),
        "past the runner's start, not in its error path"
    );
    assert!(figures < looping, "once, before the run loop");
    // Every counts tick, right after the underrun look: a cause belongs to
    // the counts beside it.
    let tick = src.find("if counts.due(now) {").expect("the counts tick");
    let tick_end = src[tick..].find("\"bridge counts\"").expect("its log line") + tick;
    let publish = once("underruns.publish(media.bridge_figures());");
    once(".publish(");
    assert!(
        (tick..tick_end).contains(&publish),
        "the publish is not inside the counts tick"
    );
    let look = once("crate::app_status::underrun_tick(");
    assert!(
        src[look..].starts_with(
            "crate::app_status::underrun_tick(\n                &mut underruns,\n                &s,\n                to_stats.underruns(),\n                from_stats.underruns(),\n                from_stats.target_ms(),\n            );\n            underruns.publish(media.bridge_figures());"
        ),
        "the publish right after the underrun look"
    );
}
