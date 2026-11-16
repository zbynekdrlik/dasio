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

fn ring(end: u64, recent: &[Wake]) -> Option<Wakes<'_>> {
    Some(Wakes { end, recent })
}

const LATE: Wake = Wake {
    behind_us: 11_000,
    rebootstrap: true,
};

/// A tracker that took its first look at zero counts and no wakes.
fn started() -> Tracker {
    let mut t = tracker();
    assert_eq!(t.take(Counts::default(), ring(0, &[])), Lost::default());
    t
}

fn lost(late_us: u64, device_us: u64) -> Lost {
    Lost { late_us, device_us }
}

#[test]
fn a_late_wake_s_skipped_blocks_are_the_environment_s_in_their_own_look() {
    let mut t = started();
    // CI 43779634868: the pump woke 11 ms late and skipped 33 blocks of
    // 333 333 ns. dstack counts the cause before the blocks, so this look
    // judges them (dstack #332).
    assert_eq!(
        t.take(counts(33, 0, 1, 1), ring(1, &[LATE])),
        lost(10_999, 0)
    );
    // The next look adds nothing.
    assert_eq!(
        t.take(counts(33, 0, 1, 1), ring(1, &[LATE])),
        lost(10_999, 0)
    );
}

#[test]
fn a_cause_seen_before_its_blocks_is_judged_with_them_in_the_next_look() {
    let mut t = started();
    // The look fell between the cause's count and the blocks': nothing
    // is judged yet.
    assert_eq!(t.take(counts(0, 0, 1, 1), ring(1, &[LATE])), lost(0, 0));
    // The blocks with no new cause: the late wake's, from the look before.
    assert_eq!(
        t.take(counts(33, 0, 1, 1), ring(1, &[LATE])),
        lost(10_999, 0)
    );
    // The same for a re-anchor: its blocks are the device's.
    let mut t = started();
    assert_eq!(t.take(counts(0, 0, 1, 0), ring(0, &[])), lost(0, 0));
    assert_eq!(t.take(counts(20, 0, 1, 0), ring(0, &[])), lost(0, 6_666));
    // And a late wake beside a re-anchor whose blocks came later: the
    // device's.
    let mut t = started();
    assert_eq!(t.take(counts(0, 0, 2, 1), ring(1, &[LATE])), lost(0, 0));
    assert_eq!(
        t.take(counts(20, 0, 2, 1), ring(1, &[LATE])),
        lost(0, 6_666)
    );
}

#[test]
fn a_re_bootstrap_that_skips_no_block_defers_one_look_only() {
    let mut t = started();
    // A re-time that skipped nothing: deferred once, then judged (no
    // blocks, nothing lost).
    assert_eq!(t.take(counts(0, 0, 1, 0), ring(0, &[])), lost(0, 0));
    assert_eq!(t.take(counts(0, 0, 1, 0), ring(0, &[])), lost(0, 0));
    // A late wake later is judged on its own: the re-time is behind the
    // baseline now.
    assert_eq!(
        t.take(counts(10, 0, 2, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
    // Two looks in a row with a cause and no blocks: the second judges.
    let mut t = started();
    assert_eq!(t.take(counts(0, 0, 1, 0), ring(0, &[])), lost(0, 0));
    assert_eq!(t.take(counts(0, 0, 2, 1), ring(1, &[LATE])), lost(0, 0));
    // Its late re-bootstrap had no blocks in its look: the blocks that
    // come now have no cause of their own, the device's. A known limit,
    // on the safe side (see the module docs): it needs a re-bootstrap with
    // no block skipped right before a look that falls between a late
    // cause's count and its blocks'.
    assert_eq!(
        t.take(counts(10, 0, 2, 1), ring(1, &[LATE])),
        lost(0, 3_333)
    );
}

#[test]
fn skipped_blocks_without_a_late_re_bootstrap_are_the_device_s() {
    let mut t = started();
    // A wake already read (its re-bootstrap not counted yet) leaves a
    // surplus; blocks then skipped with no re-bootstrap counted at all are
    // still not a late wake's.
    t.take(Counts::default(), ring(1, &[LATE]));
    assert_eq!(
        t.take(counts(10, 0, 0, 0), ring(1, &[LATE])),
        lost(0, 3_333)
    );
}

#[test]
fn an_overrun_in_the_look_makes_its_blocks_the_device_s() {
    let mut t = started();
    assert_eq!(
        t.take(counts(10, 1, 1, 1), ring(1, &[LATE])),
        lost(0, 3_333)
    );
}

#[test]
fn an_overrun_in_the_look_before_makes_the_next_look_s_blocks_the_device_s() {
    let mut t = started();
    // An overrun alone, nothing skipped.
    assert_eq!(t.take(counts(0, 1, 0, 0), ring(0, &[])), lost(0, 0));
    assert_eq!(
        t.take(counts(10, 1, 1, 1), ring(1, &[LATE])),
        lost(0, 3_333)
    );
    // A look with no overrun since: a late wake is the environment's again.
    let two = [LATE, LATE];
    assert_eq!(
        t.take(counts(20, 1, 2, 2), ring(2, &two)),
        lost(3_333, 3_333)
    );
}

#[test]
fn a_re_bootstrap_of_another_cause_makes_the_look_the_device_s() {
    let mut t = started();
    // One late and one re-anchor in the same look.
    assert_eq!(
        t.take(counts(10, 0, 2, 1), ring(1, &[LATE])),
        lost(0, 3_333)
    );
}

#[test]
fn a_wake_measured_short_of_the_margin_is_no_evidence() {
    let mut t = started();
    let short = Wake {
        behind_us: 665,
        rebootstrap: true,
    };
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[short])),
        lost(0, 3_333)
    );
    // Exactly W (666 µs) is behind the deadline.
    let at = Wake {
        behind_us: 666,
        rebootstrap: true,
    };
    let both = [short, at];
    assert_eq!(
        t.take(counts(20, 0, 2, 2), ring(2, &both)),
        lost(3_333, 3_333)
    );
}

#[test]
fn a_late_wake_without_a_re_bootstrap_is_no_evidence() {
    let mut t = started();
    let caught_up = Wake {
        behind_us: 11_000,
        rebootstrap: false,
    };
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[caught_up])),
        lost(0, 3_333)
    );
}

#[test]
fn a_torn_read_is_the_device_s_and_its_wakes_are_read_next() {
    let mut t = started();
    assert_eq!(t.take(counts(10, 0, 1, 1), None), lost(0, 3_333));
    // The next look reads both wakes once: the first re-bootstrap's and
    // the second's.
    let two = [LATE, LATE];
    assert_eq!(
        t.take(counts(20, 0, 2, 2), ring(2, &two)),
        lost(3_333, 3_333)
    );
    // Read once, not twice: a third re-bootstrap with no wake read is no
    // late wake's.
    assert_eq!(
        t.take(counts(30, 0, 3, 3), ring(2, &two)),
        lost(3_333, 6_666)
    );
}

#[test]
fn a_torn_read_is_the_device_s_even_with_wakes_read_before() {
    let mut t = started();
    // A wake read before its re-bootstrap was counted.
    t.take(Counts::default(), ring(1, &[LATE]));
    assert_eq!(t.take(counts(10, 0, 1, 1), None), lost(0, 3_333));
}

#[test]
fn wakes_past_what_was_read_are_the_device_s_then_the_looks_start_even() {
    let mut t = started();
    // Three new wakes, only the last two read (a ring overrun).
    let two = [LATE, LATE];
    assert_eq!(t.take(counts(30, 0, 3, 3), ring(3, &two)), lost(0, 9_999));
    assert_eq!(
        t.take(counts(40, 0, 4, 4), ring(4, &[LATE])),
        lost(3_333, 9_999)
    );
}

#[test]
fn every_new_wake_read_counts_when_they_fill_the_read() {
    let mut t = started();
    let two = [LATE, LATE];
    assert_eq!(t.take(counts(20, 0, 2, 2), ring(2, &two)), lost(6_666, 0));
}

#[test]
fn only_the_new_wakes_count() {
    let mut t = started();
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
    // The ring still holds the old wake; the new one fell short: no
    // evidence for the second re-bootstrap.
    let short = Wake {
        behind_us: 10,
        rebootstrap: true,
    };
    assert_eq!(
        t.take(counts(20, 0, 2, 2), ring(2, &[LATE, short])),
        lost(3_333, 3_333)
    );
}

#[test]
fn a_wake_read_before_its_re_bootstrap_was_counted_carries_to_the_next_look() {
    let mut t = started();
    // The ring already holds the wake whose re-bootstrap the counts (read
    // first) did not show yet.
    assert_eq!(t.take(Counts::default(), ring(1, &[LATE])), lost(0, 0));
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
}

#[test]
fn a_look_with_more_wakes_measured_than_counted_is_late() {
    let mut t = started();
    // A wake read before its count, then the next look reads the next
    // wake while it counts only the first re-bootstrap: measured past the
    // count, the look is late.
    t.take(Counts::default(), ring(1, &[LATE]));
    let two = [LATE, LATE];
    assert_eq!(t.take(counts(10, 0, 1, 1), ring(2, &two)), lost(3_333, 0));
}

#[test]
fn a_ring_gone_back_is_learnt_again() {
    let mut t = tracker();
    t.take(Counts::default(), ring(5, &[LATE]));
    // A new engine's ring: learnt, its blocks no one's.
    assert_eq!(t.take(counts(10, 0, 1, 1), ring(1, &[LATE])), lost(0, 0));
    assert_eq!(
        t.take(counts(20, 0, 2, 2), ring(2, &[LATE, LATE])),
        lost(3_333, 0)
    );
}

#[test]
fn the_first_look_only_learns() {
    let mut t = tracker();
    assert_eq!(t.take(counts(500, 3, 9, 9), ring(9, &[])), lost(0, 0));
    // Its late re-bootstraps count as measured: the next look's own decides.
    assert_eq!(
        t.take(counts(510, 3, 10, 10), ring(10, &[LATE])),
        lost(3_333, 0)
    );
}

#[test]
fn a_first_look_without_a_read_reads_the_ring_from_its_start() {
    let mut t = tracker();
    t.take(counts(0, 0, 0, 0), None);
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
}

#[test]
fn counts_that_went_down_are_learnt_again() {
    let base = counts(10, 5, 3, 2);
    for down in [
        counts(5, 5, 3, 2),
        counts(20, 4, 3, 2),
        counts(20, 5, 2, 2),
        counts(20, 5, 3, 1),
    ] {
        let mut t = tracker();
        t.take(base, ring(0, &[]));
        assert_eq!(t.take(down, ring(0, &[])), lost(0, 0), "{down:?}");
    }
}

#[test]
fn relearning_forgets_an_overrun_a_deferral_and_the_ring() {
    let mut t = started();
    t.take(counts(0, 1, 0, 0), ring(0, &[]));
    // A new engine: all counts back to 0; the overrun before it is gone.
    t.take(Counts::default(), ring(0, &[]));
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
    // A look deferred (a cause, no blocks), then a new engine: the
    // relearn drops the deferral, so the next look with a cause and no
    // blocks defers again rather than judging.
    assert_eq!(
        t.take(counts(10, 0, 2, 1), ring(1, &[LATE])),
        lost(3_333, 0)
    );
    // A relearn without a read starts the ring at 0.
    assert_eq!(t.take(Counts::default(), None), lost(3_333, 0));
    assert_eq!(t.take(counts(0, 0, 1, 1), ring(1, &[LATE])), lost(3_333, 0));
    assert_eq!(
        t.take(counts(10, 0, 1, 1), ring(1, &[LATE])),
        lost(6_666, 0)
    );
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
    t.take(Counts::default(), ring(0, &[]));
    let short = Wake {
        behind_us: 2_665,
        rebootstrap: true,
    };
    assert_eq!(
        t.take(counts(3, 0, 1, 1), ring(1, &[short])),
        lost(0, 3_999)
    );
    let at = Wake {
        behind_us: 2_666,
        rebootstrap: true,
    };
    let both = [short, at];
    assert_eq!(
        t.take(counts(6, 0, 2, 2), ring(2, &both)),
        lost(3_999, 3_999)
    );
}
