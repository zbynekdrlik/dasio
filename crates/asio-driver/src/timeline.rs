// SPDX-License-Identifier: GPL-3.0-or-later

//! The DAW's sample position. A sound card's position runs on its clock,
//! so the samples a host missed are in it. Ours runs on the device's media
//! time: the blocks the host missed and the blocks the device's pump
//! skipped are in it, and a round trip measured on it is the one the
//! driver reports.

/// The longest jump forward taken as time that passed: past it (a new
/// clock epoch) the position goes on one block.
pub const MAX_JUMP: u64 = 96_000;

/// Where the DAW's position stands.
#[derive(Debug, Default)]
pub struct Timeline {
    /// The last block's media start and the position given to it.
    last: Option<(u64, u64)>,
}

impl Timeline {
    pub fn new() -> Self {
        Timeline::default()
    }

    /// The position of the next block handed to the DAW. `start` is its
    /// media time (`None` without a clock), `step` how far the device's seq
    /// moved since the last one handed over (1 when none was missed),
    /// `next` the position the driver would give it (the last block's
    /// position plus a block, and any quiet blocks since).
    pub fn next(&mut self, start: Option<u64>, step: u64, next: u64, block: u64) -> u64 {
        let pos = match (start, self.last) {
            // Media time that passed: a block's, or more for the blocks the
            // host missed or the pump skipped. Never behind `next`.
            (Some(s), Some((ls, lp))) if s > ls && s - ls <= MAX_JUMP => (lp + (s - ls)).max(next),
            // No media time, or a jump back (a retime to a longer latency)
            // or a new epoch: the blocks the device published.
            _ => next + step.saturating_sub(1) * block,
        };
        self.last = start.map(|s| (s, pos));
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_the_pump_skipped_are_in_the_position() {
        let mut t = Timeline::new();
        assert_eq!(t.next(Some(10_000), 1, 0, 32), 0);
        assert_eq!(t.next(Some(10_032), 1, 32, 32), 32);
        // 8 blocks skipped by the pump, seq one apart.
        assert_eq!(t.next(Some(10_064 + 8 * 32), 1, 64, 32), 64 + 8 * 32);
        assert_eq!(
            t.next(Some(10_096 + 8 * 32), 1, 96 + 8 * 32, 32),
            96 + 8 * 32
        );
    }

    #[test]
    fn blocks_the_host_missed_are_in_the_position_with_or_without_a_clock() {
        let mut t = Timeline::new();
        assert_eq!(t.next(Some(0), 1, 0, 32), 0);
        // 14 blocks published while the host was away.
        assert_eq!(t.next(Some(15 * 32), 15, 32, 32), 15 * 32);
        let mut free = Timeline::new();
        assert_eq!(free.next(None, 1, 0, 32), 0);
        assert_eq!(free.next(None, 15, 32, 32), 15 * 32);
        assert_eq!(free.next(None, 1, 16 * 32, 32), 16 * 32);
    }

    #[test]
    fn the_position_never_goes_back_or_leaps() {
        let mut t = Timeline::new();
        assert_eq!(t.next(Some(5_000), 1, 0, 32), 0);
        // A retime to a longer latency: the inputs are older, the media
        // time steps back; the position goes on one block.
        assert_eq!(t.next(Some(4_000), 1, 32, 32), 32);
        assert_eq!(t.next(Some(4_032), 1, 64, 32), 64);
        // A new epoch far ahead: one block, not the jump.
        assert_eq!(t.next(Some(4_064 + MAX_JUMP + 1), 1, 96, 32), 96);
        // Exactly MAX_JUMP still counts as time that passed.
        let s = 4_064 + 2 * MAX_JUMP + 1;
        assert_eq!(t.next(Some(s), 1, 128, 32), 96 + MAX_JUMP);
        // Quiet blocks the driver ran on its own clock in between: never
        // behind them.
        let quiet = 96 + MAX_JUMP + 10 * 32;
        assert_eq!(t.next(Some(s + 32), 1, quiet, 32), quiet);
        // The same media time twice: on by the blocks published.
        assert_eq!(t.next(Some(s + 32), 1, quiet + 32, 32), quiet + 32);
        assert_eq!(t.next(Some(s + 32), 3, quiet + 64, 32), quiet + 128);
    }
}
