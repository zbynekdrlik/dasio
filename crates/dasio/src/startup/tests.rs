use super::*;

fn s(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

#[test]
fn the_window_is_the_first_five_minutes() {
    assert_eq!(WINDOW, s(300));
    assert_eq!(phase(Duration::ZERO), Phase::StartUp);
    assert_eq!(phase(WINDOW - Duration::from_millis(1)), Phase::StartUp);
    assert_eq!(phase(WINDOW), Phase::Running);
    assert_eq!(phase(s(3600)), Phase::Running);
    assert_eq!(Phase::StartUp.name(), "start-up");
    assert_eq!(Phase::Running.name(), "running");
}

#[test]
fn a_rise_is_the_difference_and_a_restarted_counter_is_all_new() {
    assert_eq!(rise(10, 13), 3);
    assert_eq!(rise(13, 13), 0);
    assert_eq!(rise(0, 7), 7);
    // A new driver run or section: its count started from 0 again.
    assert_eq!(rise(13, 4), 4);
    assert_eq!(rise(13, 0), 0);
}

#[test]
fn the_start_up_counts_stay_apart_and_the_running_ones_count_from_the_window_s_end() {
    // A start's counts (the numbers of dsg's boot of 23.11.2026, there over
    // ~15 min without the boot wait), then a quiet run.
    let mut split = Split::<2>::default();
    assert_eq!(split.phase(), Phase::StartUp);
    assert_eq!(split.take(s(10), [20, 30]), None);
    assert_eq!(split.shown([20, 30]), [20, 30]);
    assert_eq!(split.take(s(290), [74, 67]), None);
    assert_eq!(split.phase(), Phase::StartUp);
    assert_eq!(split.shown([74, 67]), [74, 67]);
    // The take at 300 s: its interval began in the window, so its rise is
    // the start-up's, and the window ends here.
    assert_eq!(split.take(s(300), [75, 67]), Some([75, 67]));
    assert_eq!(split.phase(), Phase::Running);
    assert_eq!(split.shown([75, 67]), [0, 0]);
    assert_eq!(split.take(s(310), [77, 67]), None, "once");
    assert_eq!(split.shown([77, 67]), [2, 0]);
    assert_eq!(split.take(s(320), [77, 70]), None);
    assert_eq!(split.shown([77, 70]), [2, 3]);
    assert_eq!(
        split.part(),
        Part {
            in_window: false,
            startup: [75, 67]
        }
    );
}

#[test]
fn a_status_read_shows_the_rise_since_the_last_take_without_taking_it() {
    let mut split = Split::<1>::default();
    split.take(s(10), [5]);
    assert_eq!(split.shown([9]), [9], "the start-up's 5 and the live 4");
    assert_eq!(split.shown([9]), [9], "nothing was taken");
    split.take(s(20), [6]);
    assert_eq!(split.shown([6]), [6]);
    split.take(s(310), [6]);
    split.take(s(320), [8]);
    assert_eq!(split.shown([11]), [5], "the running 2 and the live 3");
    // A live counter that restarted since the last take: all new.
    assert_eq!(split.shown([1]), [3]);
}

#[test]
fn a_counter_that_restarts_after_the_window_is_counted_whole() {
    // The runner's counts are per driver run: a reopen starts them at 0.
    let mut split = Split::<1>::default();
    split.take(s(10), [40]);
    split.take(s(310), [40]);
    assert_eq!(split.shown([40]), [0]);
    split.take(s(320), [3]);
    assert_eq!(split.shown([3]), [3], "the new run's 3, never 3 - 40");
    split.take(s(330), [5]);
    assert_eq!(split.shown([5]), [5]);
    assert_eq!(split.part().startup, [40]);
}

#[test]
fn the_status_part_is_in_the_window_first_and_the_start_up_counts_after() {
    let mut split = Split::<2>::default();
    assert_eq!(
        split.part(),
        Part {
            in_window: true,
            startup: [0, 0]
        }
    );
    split.take(s(10), [3, 1]);
    assert!(split.part().in_window);
    split.take(s(301), [4, 1]);
    assert_eq!(
        split.part(),
        Part {
            in_window: false,
            startup: [4, 1]
        }
    );
    assert_eq!(
        Part::<2>::default(),
        Part {
            in_window: false,
            startup: [0, 0]
        },
        "a status with no split shows no start-up part"
    );
}

#[test]
fn a_start_that_never_reaches_the_window_s_end_reports_no_end() {
    let mut split = Split::<1>::default();
    for t in [10, 100, 200, 299] {
        assert_eq!(split.take(s(t), [t]), None);
    }
    assert_eq!(split.shown([299]), [299]);
}

#[test]
fn a_count_near_the_top_saturates_instead_of_wrapping() {
    let mut split = Split::<1>::default();
    split.take(s(10), [u64::MAX]);
    split.take(s(20), [0]);
    split.take(s(30), [5]);
    assert_eq!(split.shown([5]), [u64::MAX]);
}

#[test]
fn a_tick_takes_and_shows_the_phase_s_counts() {
    let mut split = Split::<2>::default();
    assert_eq!(
        split.tick(s(10), [3, 1]),
        Tick {
            phase: Phase::StartUp,
            shown: [3, 1],
            ended: None
        }
    );
    assert_eq!(
        split.tick(s(300), [4, 1]),
        Tick {
            phase: Phase::Running,
            shown: [0, 0],
            ended: Some([4, 1])
        }
    );
    assert_eq!(
        split.tick(s(310), [6, 2]),
        Tick {
            phase: Phase::Running,
            shown: [2, 1],
            ended: None
        }
    );
}

#[test]
fn a_restarted_window_counts_from_the_last_take_and_ends_again() {
    // A DAW that attaches at 1000 s (seen at the 1010 s tick): its start's
    // counts are start-up counts too (owner 21.11.2026).
    let mut split = Split::<1>::default();
    split.tick(s(10), [5]);
    split.tick(s(300), [5]);
    split.tick(s(1000), [7]);
    assert_eq!(split.shown([7]), [2], "running: 2");
    split.restart_window();
    assert_eq!(split.phase(), Phase::StartUp, "a new window from 1000 s");
    assert!(split.part().in_window);
    // The interval since the last take is the new window's: 5 from the
    // first window and 3 from this one.
    let t = split.tick(s(1010), [10]);
    assert_eq!((t.phase, t.shown, t.ended), (Phase::StartUp, [8], None));
    let t = split.tick(s(1290), [11]);
    assert_eq!((t.phase, t.shown, t.ended), (Phase::StartUp, [9], None));
    let t = split.tick(s(1300), [11]);
    assert_eq!(
        (t.phase, t.shown, t.ended),
        (Phase::Running, [2], Some([9])),
        "the window ends 300 s after it began; the running 2 stay"
    );
    assert_eq!(split.part().startup, [9]);
}

#[test]
fn a_restart_within_the_window_only_prolongs_it() {
    let mut split = Split::<1>::default();
    split.tick(s(100), [1]);
    split.restart_window();
    assert_eq!(split.tick(s(300), [2]).ended, None, "not yet: from 100 s");
    assert_eq!(split.tick(s(390), [2]).ended, None);
    assert_eq!(split.tick(s(400), [3]).ended, Some([3]));
}

/// The Windows glue that feeds the split (read as text: it is not built
/// on Linux). A Windows checkout has CRLF lines.
fn glue(src: &str) -> String {
    src.replace("\r\n", "\n")
}

fn once(src: &str, needle: &str) -> usize {
    let found: Vec<usize> = src.match_indices(needle).map(|(i, _)| i).collect();
    assert_eq!(found.len(), 1, "{needle}: once");
    found[0]
}

#[test]
fn the_daw_kind_s_counts_tick_feeds_the_split_and_logs_its_phase() {
    let run = glue(include_str!("../run.rs"));
    let tick = once(&run, "if counts.due(now) {");
    let call = once(
        &run,
        "phase_counts.tick(start.elapsed(), &serving.current, converter_dry, &v, lost);",
    );
    assert!(tick < call, "in the counts tick");
    once(&run, "phase_counts.header(),");
    let counts = glue(include_str!("../run/counts.rs"));
    let attach = once(
        &counts,
        "if self.attach.opened(attached, current.shared.session()) {",
    );
    let restart = once(&counts, "header.restart_window();");
    once(&counts, "self.pump.restart_window();");
    let header = once(
        &counts,
        "let header_tick = header.tick(since, header_totals);",
    );
    assert!(
        attach < restart && restart < header,
        "a new window before the take"
    );
    once(
        &counts,
        "let pump_tick = self.pump.tick(since, pump_totals);",
    );
    let ended = once(&counts, "startup_ended(header_startup, pump_startup);");
    let line = once(&counts, "phase = header_tick.phase.name(),");
    let end = once(&counts, "\"counts\"\n        );");
    assert!(ended < line && line < end, "the end, then the counts line");
    once(&counts, "let header_totals = header_totals(current);");
    let serve = glue(include_str!("../run/serve.rs"));
    once(&serve, "(split.shown(header_totals(&c)), split.part())");
    once(&serve, "startup: part,");
}

#[test]
fn the_bridge_s_counts_tick_and_status_read_the_split() {
    let src = glue(include_str!("../run_bridge.rs"));
    let tick = once(&src, "if counts.due(now) {");
    let call = once(&src, "let c = phase_counts.tick(");
    let phase = once(&src, "phase = c.phase.name(),");
    let end = once(&src, "\"bridge counts\"");
    assert!(
        tick < call && call < phase && phase < end,
        "in the counts tick"
    );
    once(&src, "phase: phase_counts.status(),");
    once(
        &src,
        "let shown = lock(phase).shown(&s, to_stats.underruns(), from_stats.underruns());",
    );
    for field in [
        "resets: shown.resets,",
        "late_bins: shown.late_bins,",
        "startup: shown.startup,",
    ] {
        once(&src, field);
    }
    let counts = glue(include_str!("../run_bridge/counts.rs"));
    once(&counts, "startup_ended(status, bins, line);");
}

#[test]
fn a_daw_that_attaches_or_comes_back_on_a_new_section_opens_a_window() {
    let mut a = Attach::default();
    assert!(!a.opened(false, 7), "no DAW");
    assert!(a.opened(true, 7), "it attached");
    assert!(!a.opened(true, 7), "still attached");
    assert!(!a.opened(false, 7), "it stopped");
    assert!(a.opened(true, 7), "it started again");
    // A rate change: a new section, the DAW back on it within a tick.
    assert!(a.opened(true, 8), "back on a new section");
    assert!(!a.opened(true, 8));
    // A new section with no DAW on it yet, then the DAW.
    assert!(!a.opened(false, 9));
    assert!(a.opened(true, 9));
    // A DAW attached at the first look: a window from the start, which the
    // start's window already is.
    assert!(Attach::default().opened(true, 0));
    assert!(Attach::default().opened(true, 3));
}
