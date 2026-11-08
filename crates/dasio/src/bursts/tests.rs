use super::*;

#[test]
fn the_first_look_sets_the_base_and_says_nothing() {
    let mut b = Bursts::default();
    assert_eq!(b.tick(0, 90, 100), None);
    assert_eq!(b.tick(100, 90, 100), None, "nothing moved");
}

#[test]
fn a_rise_starts_a_burst_once_and_its_end_sums_it() {
    let mut b = Bursts::default();
    b.tick(0, 49, 58);
    assert_eq!(
        b.tick(100, 60, 70),
        Some(Event::Start {
            missed: 11,
            late: 12
        }),
        "the first tick that sees the rise"
    );
    assert_eq!(
        b.tick(200, 87, 96),
        None,
        "still rising: one line per burst"
    );
    assert_eq!(
        b.tick(300, 87, 96),
        Some(Event::End {
            missed: 38,
            late: 38,
            ms: 200
        }),
        "the first quiet tick ends it with the burst's totals"
    );
    assert_eq!(b.tick(400, 87, 96), None);
}

#[test]
fn a_late_block_alone_is_a_burst_too() {
    let mut b = Bursts::default();
    b.tick(0, 5, 5);
    assert_eq!(b.tick(100, 5, 6), Some(Event::Start { missed: 0, late: 1 }));
    assert_eq!(
        b.tick(200, 5, 6),
        Some(Event::End {
            missed: 0,
            late: 1,
            ms: 100
        })
    );
}

#[test]
fn a_counter_that_went_down_starts_over_without_a_burst() {
    // The DAW reattached and the shared memory's counters restarted.
    let mut b = Bursts::default();
    b.tick(0, 90, 100);
    b.tick(100, 95, 100);
    assert_eq!(b.tick(200, 0, 0), None, "a restart, not a burst");
    assert_eq!(b.tick(300, 0, 0), None);
    assert_eq!(b.tick(400, 2, 0), Some(Event::Start { missed: 2, late: 0 }));
}

#[test]
fn one_counter_going_down_is_a_restart_even_if_the_other_rose() {
    let mut b = Bursts::default();
    b.tick(0, 90, 100);
    assert_eq!(b.tick(100, 0, 105), None, "a restart, not a burst");
}
