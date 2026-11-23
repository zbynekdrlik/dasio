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
    // dsg 23.11.2026: a boot's 74 late callbacks and 67 underruns, then a
    // quiet run.
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
