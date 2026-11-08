use super::*;

/// `n` quiet ticks every 100 ms after `from` ms; the events they gave.
fn quiet(b: &mut Bursts, from: u64, n: u32, missed: u64, late: u64) -> Vec<Event> {
    (1..=u64::from(n))
        .filter_map(|i| b.tick(from + 100 * i, missed, late))
        .collect()
}

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
        quiet(&mut b, 200, QUIET_TICKS - 1, 87, 96),
        vec![],
        "not over before QUIET_TICKS quiet ticks"
    );
    assert_eq!(
        b.tick(200 + 100 * u64::from(QUIET_TICKS), 87, 96),
        Some(Event::End {
            missed: 38,
            late: 38,
            ms: 100
        }),
        "the burst's totals, first rise to last"
    );
    assert_eq!(quiet(&mut b, 1_200, 20, 87, 96), vec![], "nothing more");
}

#[test]
fn misses_that_come_and_go_within_the_quiet_time_are_one_burst() {
    let mut b = Bursts::default();
    b.tick(0, 0, 0);
    let mut events = Vec::new();
    // A miss every 3rd tick for 3 s: one Start, no End meanwhile.
    for i in 1..=30u64 {
        let n = i / 3;
        events.extend(b.tick(100 * i, n, n));
    }
    assert_eq!(events.len(), 1, "one line while it goes on: {events:?}");
    assert_eq!(events[0], Event::Start { missed: 1, late: 1 }, "at 300 ms");
    let end = quiet(&mut b, 3_000, QUIET_TICKS, 10, 10);
    assert_eq!(
        end,
        vec![Event::End {
            missed: 10,
            late: 10,
            ms: 2_700
        }]
    );
}

#[test]
fn a_late_block_alone_is_a_burst_too() {
    let mut b = Bursts::default();
    b.tick(0, 5, 5);
    assert_eq!(b.tick(100, 5, 6), Some(Event::Start { missed: 0, late: 1 }));
    assert_eq!(
        quiet(&mut b, 100, QUIET_TICKS, 5, 6),
        vec![Event::End {
            missed: 0,
            late: 1,
            ms: 0
        }]
    );
}

#[test]
fn a_restart_ends_an_open_burst_with_what_it_counted() {
    // The DAW reattached and the shared memory's counters restarted.
    let mut b = Bursts::default();
    b.tick(0, 90, 100);
    b.tick(100, 95, 100);
    b.tick(200, 97, 100);
    assert_eq!(
        b.tick(300, 0, 0),
        Some(Event::End {
            missed: 7,
            late: 0,
            ms: 100
        }),
        "the open burst ends at the restart"
    );
    assert_eq!(b.tick(400, 0, 0), None);
    assert_eq!(b.tick(500, 2, 0), Some(Event::Start { missed: 2, late: 0 }));
}

#[test]
fn a_restart_with_no_burst_open_says_nothing() {
    let mut b = Bursts::default();
    b.tick(0, 90, 100);
    assert_eq!(b.tick(100, 0, 105), None, "a restart, not a burst");
}
