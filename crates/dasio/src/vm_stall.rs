//! The probe's VM-stall accounting (dasio#114): which of the host's gaps
//! and lost impulses a stop of the whole guest explains.
//!
//! The probe's host is driven by the device, so a device-only stall shows
//! as a host gap too. A watchdog thread on its own timer tells them apart:
//! a VM stop delays the watchdog and the switches together, a device stall
//! only the switches. Every span here is in µs since the probe's start,
//! `(start, end)` with `start <= end`.

/// A watchdog wake later than this past its due time is a stall, µs: the
/// same 1 ms the probe's `gaps_over_1ms` counts the host's gaps by.
pub const STALL_US: u64 = 1_000;

/// The stall a watchdog wake shows: `Some((due, woke))` when it woke more
/// than [`STALL_US`] after it was due, else `None`.
pub fn stall_of(due_us: u64, woke_us: u64) -> Option<(u64, u64)> {
    (woke_us > due_us + STALL_US).then_some((due_us, woke_us))
}

/// Whether `span` and any of `stalls` share a moment (touching ends count:
/// a switch that came the instant the guest resumed was held by it).
pub fn overlaps(span: (u64, u64), stalls: &[(u64, u64)]) -> bool {
    stalls.iter().any(|&(s, e)| s <= span.1 && span.0 <= e)
}

/// How many of `ways` (a lost impulse's emit to the next emit) had a host
/// gap end inside them: after the emit, up to and with the next. A gap that
/// ended at the emit's own switch came before the impulse left (dasio#114,
/// the rule the probe's 1 ms gap counter kept before).
pub fn ended_within(ways: &[(u64, u64)], ends: &[u64]) -> u64 {
    ways.iter()
        .filter(|&&(from, to)| ends.iter().any(|&e| from < e && e <= to))
        .count() as u64
}

/// The blocks of the host's gaps that overlap a stall: each gap is
/// `((start, end), blocks)`, its blocks as `probe::blocks_in_gap` counts
/// them.
pub fn explained_blocks(gaps: &[((u64, u64), u64)], stalls: &[(u64, u64)]) -> u64 {
    gaps.iter()
        .filter(|(sp, _)| overlaps(*sp, stalls))
        .map(|&(_, blocks)| blocks)
        .sum()
}

/// The blocks of `block_us` the stalls span, each rounded up: the switches
/// a stopped guest could not take.
pub fn stall_blocks(stalls: &[(u64, u64)], block_us: u64) -> u64 {
    if block_us == 0 {
        return 0;
    }
    stalls
        .iter()
        .map(|&(s, e)| (e - s).div_ceil(block_us))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_up_to_one_ms_late_is_no_stall() {
        assert_eq!(stall_of(1_000, 1_000), None);
        assert_eq!(stall_of(1_000, 2_000), None);
        assert_eq!(stall_of(1_000, 2_001), Some((1_000, 2_001)));
        // CI 43579913777: the guest stopped ~16 ms.
        assert_eq!(stall_of(5_000, 21_000), Some((5_000, 21_000)));
    }

    #[test]
    fn spans_overlap_when_they_share_a_moment() {
        let stalls = [(100, 200), (1_000, 1_500)];
        assert!(overlaps((150, 160), &stalls));
        assert!(overlaps((50, 100), &stalls));
        assert!(overlaps((200, 300), &stalls));
        assert!(overlaps((1_400, 2_000), &stalls));
        assert!(!overlaps((201, 999), &stalls));
        assert!(!overlaps((0, 99), &stalls));
        assert!(!overlaps((1_501, 1_600), &stalls));
        assert!(!overlaps((150, 160), &[]));
    }

    #[test]
    fn a_gap_counts_for_a_lost_impulse_when_it_ends_on_its_way() {
        let ways = [(1_000, 2_000), (5_000, 6_000)];
        // Ended at the emit itself: before the impulse left.
        assert_eq!(ended_within(&ways, &[1_000]), 0);
        assert_eq!(ended_within(&ways, &[1_001]), 1);
        // Ended at the next emit's switch: on its way.
        assert_eq!(ended_within(&ways, &[2_000]), 1);
        assert_eq!(ended_within(&ways, &[2_001]), 0);
        assert_eq!(ended_within(&ways, &[1_500, 5_500]), 2);
        assert_eq!(ended_within(&ways, &[1_500, 1_600]), 1);
        assert_eq!(ended_within(&ways, &[]), 0);
        assert_eq!(ended_within(&[], &[1_500]), 0);
    }

    #[test]
    fn a_device_stall_explains_no_blocks() {
        let stalls = [(10_000, 26_000)];
        // A host gap inside the guest's stop: its blocks are the runner's.
        // One elsewhere (the device stalled alone): none.
        let gaps = [((9_900, 26_100), 49), ((40_000, 42_000), 6)];
        assert_eq!(explained_blocks(&gaps, &stalls), 49);
        assert_eq!(explained_blocks(&gaps[1..], &stalls), 0);
        assert_eq!(explained_blocks(&gaps, &[]), 0);
    }

    #[test]
    fn stall_blocks_round_each_stall_up() {
        // Block 32 at 96 kHz: 333 us.
        assert_eq!(stall_blocks(&[(0, 333)], 333), 1);
        assert_eq!(stall_blocks(&[(0, 334)], 333), 2);
        assert_eq!(stall_blocks(&[(0, 16_000), (20_000, 20_001)], 333), 49 + 1);
        assert_eq!(stall_blocks(&[], 333), 0);
        assert_eq!(stall_blocks(&[(0, 16_000)], 0), 0);
    }
}
