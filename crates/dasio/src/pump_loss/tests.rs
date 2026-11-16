use super::*;

/// Block 32 and W 64 at 96 kHz: a block is 333 333 ns, W 666 µs.
fn tracker() -> Tracker {
    Tracker::new(32, 64, 96_000)
}

fn counts(missed: u64, overruns: u64, rebootstraps: u64, late: u64) -> Counts {
    Counts {
        missed,
        overruns,
        rebootstraps,
        rebootstraps_late: late,
    }
}

const LATE: Wake = Wake {
    behind_us: 11_000,
    rebootstrap: true,
};

/// A tracker that took its first look at zero counts and no wakes.
fn started() -> Tracker {
    let mut t = tracker();
    assert_eq!(
        t.take(
            Counts::default(),
            Some(Wakes {
                end: 0,
                recent: &[]
            })
        ),
        Lost::default()
    );
    t
}

#[test]
fn a_late_wake_s_skipped_blocks_are_the_environment_s() {
    let mut t = started();
    // CI 43779634868: the pump woke 11 ms late and skipped 33 blocks.
    let lost = t.take(
        counts(33, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    // 33 blocks of 333 333 ns.
    assert_eq!(
        lost,
        Lost {
            late_us: 10_999,
            device_us: 0
        }
    );
}

#[test]
fn an_overrun_in_the_look_makes_its_blocks_the_device_s() {
    let mut t = started();
    let lost = t.take(
        counts(10, 1, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(
        lost,
        Lost {
            late_us: 0,
            device_us: 3_333
        }
    );
}

#[test]
fn an_overrun_in_the_look_before_makes_the_next_look_s_blocks_the_device_s() {
    let mut t = started();
    // An overrun alone, nothing skipped.
    assert_eq!(
        t.take(
            counts(0, 1, 0, 0),
            Some(Wakes {
                end: 0,
                recent: &[]
            })
        ),
        Lost::default()
    );
    let lost = t.take(
        counts(10, 1, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.device_us, 3_333);
    assert_eq!(lost.late_us, 0);
    // A look with no overrun since: a late wake is the environment's again.
    let lost = t.take(
        counts(20, 1, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[LATE, LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn a_re_bootstrap_of_another_cause_makes_the_look_the_device_s() {
    let mut t = started();
    // One late and one re-anchor in the same look.
    let lost = t.take(
        counts(10, 0, 2, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 0);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn a_wake_measured_short_of_the_margin_is_no_evidence() {
    let mut t = started();
    let short = Wake {
        behind_us: 665,
        rebootstrap: true,
    };
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[short],
        }),
    );
    assert_eq!(lost.late_us, 0);
    assert_eq!(lost.device_us, 3_333);
    // Exactly W (666 µs) is behind the deadline.
    let at = Wake {
        behind_us: 666,
        rebootstrap: true,
    };
    let lost = t.take(
        counts(20, 0, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[short, at],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn a_late_wake_without_a_re_bootstrap_is_no_evidence() {
    let mut t = started();
    let caught_up = Wake {
        behind_us: 11_000,
        rebootstrap: false,
    };
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[caught_up],
        }),
    );
    assert_eq!(lost.late_us, 0);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn a_torn_read_is_the_device_s_and_its_wakes_are_read_next() {
    let mut t = started();
    let lost = t.take(counts(10, 0, 1, 1), None);
    assert_eq!(lost.device_us, 3_333);
    // The next look reads both wakes: the second re-bootstrap's and the
    // first's (settled already).
    let lost = t.take(
        counts(20, 0, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[LATE, LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn wakes_past_what_was_read_are_the_device_s_then_the_looks_start_even() {
    let mut t = started();
    // Three new wakes, only the last two read (a ring overrun).
    let lost = t.take(
        counts(30, 0, 3, 3),
        Some(Wakes {
            end: 3,
            recent: &[LATE, LATE],
        }),
    );
    assert_eq!(lost.late_us, 0);
    assert_eq!(lost.device_us, 9_999);
    let lost = t.take(
        counts(40, 0, 4, 4),
        Some(Wakes {
            end: 4,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn every_new_wake_read_counts_when_they_fill_the_read() {
    let mut t = started();
    let lost = t.take(
        counts(20, 0, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[LATE, LATE],
        }),
    );
    assert_eq!(lost.late_us, 6_666);
    assert_eq!(lost.device_us, 0);
}

#[test]
fn only_the_new_wakes_count() {
    let mut t = started();
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
    // The ring still holds the old wake; the new one fell short: no
    // evidence for the second re-bootstrap.
    let short = Wake {
        behind_us: 10,
        rebootstrap: true,
    };
    let lost = t.take(
        counts(20, 0, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[LATE, short],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn a_wake_read_before_its_re_bootstrap_was_counted_carries_to_the_next_look() {
    let mut t = started();
    // The ring already holds the wake whose re-bootstrap the counts (read
    // first) did not show yet.
    assert_eq!(
        t.take(
            Counts::default(),
            Some(Wakes {
                end: 1,
                recent: &[LATE]
            })
        ),
        Lost::default()
    );
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn a_ring_behind_the_last_read_is_the_device_s() {
    let mut t = tracker();
    t.take(
        Counts::default(),
        Some(Wakes {
            end: 5,
            recent: &[LATE],
        }),
    );
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 4,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 0);
    assert_eq!(lost.device_us, 3_333);
}

#[test]
fn the_first_look_only_learns() {
    let mut t = tracker();
    assert_eq!(
        t.take(
            counts(500, 3, 9, 9),
            Some(Wakes {
                end: 9,
                recent: &[]
            })
        ),
        Lost::default()
    );
    // Its late re-bootstraps count as measured: the next look's own decides.
    let lost = t.take(
        counts(510, 3, 10, 10),
        Some(Wakes {
            end: 10,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn a_first_look_without_a_read_keeps_the_ring_unread() {
    let mut t = tracker();
    t.take(counts(0, 0, 0, 0), None);
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn counts_that_went_down_are_learnt_again() {
    let base = counts(10, 5, 3, 2);
    let wakes = || {
        Some(Wakes {
            end: 0,
            recent: &[],
        })
    };
    for down in [
        counts(5, 5, 3, 2),
        counts(20, 4, 3, 2),
        counts(20, 5, 2, 2),
        counts(20, 5, 3, 1),
    ] {
        let mut t = tracker();
        t.take(base, wakes());
        assert_eq!(t.take(down, wakes()), Lost::default(), "{down:?}");
    }
}

#[test]
fn relearning_forgets_an_overrun() {
    let mut t = started();
    t.take(
        counts(0, 1, 0, 0),
        Some(Wakes {
            end: 0,
            recent: &[],
        }),
    );
    // A new engine: all counts back to 0.
    t.take(
        Counts::default(),
        Some(Wakes {
            end: 0,
            recent: &[],
        }),
    );
    let lost = t.take(
        counts(10, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[LATE],
        }),
    );
    assert_eq!(lost.late_us, 3_333);
}

#[test]
fn a_ring_read_is_whole_only_when_nothing_moved_or_was_left_out() {
    let two = [LATE, LATE];
    assert!(Wakes::whole(2, &two, 2, 32).is_some());
    // The pump wrote a wake during the read.
    assert!(Wakes::whole(2, &two, 3, 32).is_none());
    // A slot left out.
    assert!(Wakes::whole(3, &two, 3, 32).is_none());
    // Past the ring's size, it holds its size.
    assert!(Wakes::whole(40, &two, 40, 2).is_some());
    assert!(Wakes::whole(40, &two, 40, 3).is_none());
    let w = Wakes::whole(2, &two, 2, 32).unwrap();
    assert_eq!(w.end, 2);
    assert_eq!(w.recent.len(), 2);
}

#[test]
fn lost_time_is_whole_host_blocks_rounded_down() {
    // 10 999 µs at 96 kHz, block 32: 1055.9 samples, 32 whole blocks.
    assert_eq!(blocks_of(10_999, 96_000, 32), 32);
    assert_eq!(blocks_of(1_000_000, 96_000, 32), 3_000);
    // At 44.1 kHz on a 15-frame block: 44 100 / 15.
    assert_eq!(blocks_of(1_000_000, 44_100, 15), 2_940);
    assert_eq!(blocks_of(332, 96_000, 32), 0);
    assert_eq!(blocks_of(334, 96_000, 32), 1);
    assert_eq!(blocks_of(5_000, 96_000, 0), 0);
}

#[test]
fn the_tracker_s_units_follow_its_rate() {
    // Block 64 and W 128 at 48 kHz: 1 333 333 ns a block, W 2666 µs.
    let mut t = Tracker::new(64, 128, 48_000);
    t.take(
        Counts::default(),
        Some(Wakes {
            end: 0,
            recent: &[],
        }),
    );
    let short = Wake {
        behind_us: 2_665,
        rebootstrap: true,
    };
    let lost = t.take(
        counts(3, 0, 1, 1),
        Some(Wakes {
            end: 1,
            recent: &[short],
        }),
    );
    assert_eq!(lost.device_us, 3_999);
    let at = Wake {
        behind_us: 2_666,
        rebootstrap: true,
    };
    let lost = t.take(
        counts(6, 0, 2, 2),
        Some(Wakes {
            end: 2,
            recent: &[short, at],
        }),
    );
    assert_eq!(lost.late_us, 3_999);
}
