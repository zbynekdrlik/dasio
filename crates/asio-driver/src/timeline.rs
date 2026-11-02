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
        let pos = next + step.saturating_sub(1) * block;
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
}
