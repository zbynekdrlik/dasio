//! The RX-stall verdict on the output watch's held events (dasio#207): the
//! portable decision behind `run/watch_log.rs`'s lines (the Windows glue
//! only logs what it says). A zero/repeat break or a program jump drained
//! at tick T is held two ticks ([`Hold`]) and judged at T + 2 against the
//! RX gaps of ticks T - 1 to T + 2 ([`Stall`]): a stall's cut is labelled
//! [`Label::Stall`], any other one [`Label::Normal`]. A held line comes out
//! [`TICK_MS`] x ticks late: `held_ms` says how much.

use super::stall::{HOLD, Hold, Stall};
use super::{Break, ProgramJump};

/// Milliseconds of one tick of the run loop.
pub const TICK_MS: u64 = 100;

/// `ticks` ticks in ms.
fn ticks_ms(ticks: usize) -> u64 {
    ticks as u64 * TICK_MS
}

/// The items of a flush and how long the oldest was held at this last look
/// (its ticks in the hold plus the look), ms; 0 for none.
fn split<T>(held: Vec<(T, usize)>) -> (Vec<T>, u64) {
    let oldest = held.iter().map(|(_, age)| age + 1).max().unwrap_or(0);
    (held.into_iter().map(|(t, _)| t).collect(), ticks_ms(oldest))
}

/// What a due event is logged as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// A host's miss (a `broke` / `jumped` line).
    Normal,
    /// Cut by an RX stall: several sources lost packets around it.
    Stall,
}

/// The label for a judged event given whether the stall window held several
/// sources.
pub fn verdict(stalled: bool) -> Label {
    if stalled { Label::Stall } else { Label::Normal }
}

/// The events due at one tick, all with one label.
#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub label: Label,
    /// Sources that lost packets in the window.
    pub sources: u32,
    pub breaks: Vec<Break>,
    /// How long the oldest of the breaks was held, ms.
    pub break_held_ms: u64,
    pub jumps: Vec<ProgramJump>,
    /// How long the oldest of the jumps was held, ms.
    pub jump_held_ms: u64,
}

impl Due {
    /// Nothing due.
    pub fn is_empty(&self) -> bool {
        self.breaks.is_empty() && self.jumps.is_empty()
    }
}

/// The stall window and the two holds.
#[derive(Debug, Default)]
pub struct Judge {
    stall: Stall,
    breaks: Hold<Break>,
    jumps: Hold<ProgramJump>,
}

impl Judge {
    /// One tick: each RX slot's gap count and flow id ([`Stall::note`]),
    /// the breaks and jumps drained now; returns what is due.
    pub fn tick(
        &mut self,
        gaps: &[u64],
        flows: &[u64],
        breaks: Vec<Break>,
        jumps: Vec<ProgramJump>,
    ) -> Due {
        self.stall.note(gaps, flows);
        let breaks = self.breaks.tick(breaks);
        let jumps = self.jumps.tick(jumps);
        let held = |n: usize| if n == 0 { 0 } else { ticks_ms(HOLD) };
        let (bh, jh) = (held(breaks.len()), held(jumps.len()));
        self.due(breaks, jumps, bh, jh)
    }

    /// The device loop ends: one more look at the gaps and flows, then
    /// everything still held is due.
    pub fn flush(&mut self, gaps: &[u64], flows: &[u64]) -> Due {
        self.stall.note(gaps, flows);
        let (breaks, bh) = split(self.breaks.flush());
        let (jumps, jh) = split(self.jumps.flush());
        self.due(breaks, jumps, bh, jh)
    }

    fn due(
        &self,
        breaks: Vec<Break>,
        jumps: Vec<ProgramJump>,
        break_held_ms: u64,
        jump_held_ms: u64,
    ) -> Due {
        Due {
            label: verdict(self.stall.stalled()),
            sources: self.stall.sources(),
            breaks,
            break_held_ms,
            jumps,
            jump_held_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miss_watch::Kind;

    fn brk(at: u64) -> Break {
        Break {
            channel: 0,
            kind: Kind::Zero,
            at,
            len: 128,
        }
    }

    fn jump(at: u64) -> ProgramJump {
        ProgramJump {
            channel: 0,
            at,
            ratio: 30.0,
            steps: 5000,
        }
    }

    /// Cumulative gap counts of two slots after `a` and `b` gaps.
    fn g(a: u64, b: u64) -> [u64; 2] {
        [a, b]
    }

    #[test]
    fn the_verdict_follows_the_stall() {
        assert_eq!(verdict(false), Label::Normal);
        assert_eq!(verdict(true), Label::Stall);
    }

    #[test]
    fn a_break_with_no_loss_comes_out_two_ticks_later_as_normal() {
        let mut j = Judge::default();
        assert!(j.tick(&g(0, 0), &[], vec![brk(10)], vec![]).is_empty());
        assert!(j.tick(&g(0, 0), &[], vec![], vec![]).is_empty());
        let d = j.tick(&g(0, 0), &[], vec![], vec![]);
        assert_eq!(d.breaks, vec![brk(10)]);
        assert_eq!(d.label, Label::Normal);
        assert_eq!(d.sources, 0);
        assert_eq!(d.break_held_ms, 200);
        assert_eq!(d.jump_held_ms, 0);
        assert!(j.tick(&g(0, 0), &[], vec![], vec![]).is_empty());
    }

    #[test]
    fn a_jump_with_no_loss_is_normal_too_and_held_alike() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![], vec![jump(10)]);
        j.tick(&g(0, 0), &[], vec![], vec![]);
        let d = j.tick(&g(0, 0), &[], vec![], vec![]);
        assert_eq!(d.jumps, vec![jump(10)]);
        assert_eq!(d.label, Label::Normal);
        assert_eq!(d.jump_held_ms, 200);
        assert_eq!(d.break_held_ms, 0);
        assert!(d.breaks.is_empty());
    }

    #[test]
    fn gaps_confirmed_two_ticks_late_still_make_it_a_stall() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![brk(10)], vec![jump(11)]);
        j.tick(&g(0, 0), &[], vec![], vec![]);
        let d = j.tick(&g(1, 1), &[], vec![], vec![]);
        assert_eq!(d.label, Label::Stall);
        assert_eq!(d.sources, 2);
        assert_eq!(d.breaks, vec![brk(10)]);
        assert_eq!(d.jumps, vec![jump(11)]);
    }

    #[test]
    fn gaps_from_the_tick_before_the_event_count_too() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![], vec![]);
        j.tick(&g(1, 1), &[], vec![brk(10)], vec![]);
        j.tick(&g(1, 1), &[], vec![], vec![]);
        let d = j.tick(&g(1, 1), &[], vec![], vec![]);
        assert_eq!(d.label, Label::Stall);
        assert_eq!(d.breaks, vec![brk(10)]);
    }

    #[test]
    fn gaps_three_ticks_after_the_event_are_too_late() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![brk(10)], vec![]);
        j.tick(&g(0, 0), &[], vec![], vec![]);
        let d = j.tick(&g(0, 0), &[], vec![], vec![]);
        assert_eq!(d.label, Label::Normal);
        let d = j.tick(&g(1, 1), &[], vec![], vec![]);
        assert!(d.is_empty());
    }

    #[test]
    fn one_source_losing_packets_is_no_stall() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![brk(10)], vec![]);
        j.tick(&g(3, 0), &[], vec![], vec![]);
        let d = j.tick(&g(4, 0), &[], vec![], vec![]);
        assert_eq!(d.label, Label::Normal);
        assert_eq!(d.sources, 1);
    }

    #[test]
    fn a_flush_judges_after_one_more_look_and_says_how_long_each_was_held() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![brk(10)], vec![]);
        j.tick(&g(0, 0), &[], vec![], vec![jump(20)]);
        let d = j.flush(&g(2, 2), &[]);
        assert_eq!(d.label, Label::Stall);
        assert_eq!(d.sources, 2);
        assert_eq!(d.breaks, vec![brk(10)]);
        assert_eq!(d.break_held_ms, 200);
        assert_eq!(d.jumps, vec![jump(20)]);
        assert_eq!(d.jump_held_ms, 100);
        assert!(j.flush(&g(2, 2), &[]).is_empty());
    }

    #[test]
    fn a_flush_with_no_loss_is_normal_and_with_nothing_held_is_empty() {
        let mut j = Judge::default();
        assert!(j.flush(&g(0, 0), &[]).is_empty());
        j.tick(&g(0, 0), &[], vec![brk(10)], vec![]);
        let d = j.flush(&g(0, 0), &[]);
        assert_eq!(d.label, Label::Normal);
        assert_eq!(d.breaks, vec![brk(10)]);
        assert_eq!(d.break_held_ms, 100);
    }

    #[test]
    fn breaks_and_jumps_are_held_apart() {
        let mut j = Judge::default();
        j.tick(&g(0, 0), &[], vec![brk(1)], vec![]);
        j.tick(&g(0, 0), &[], vec![], vec![jump(2)]);
        let d = j.tick(&g(0, 0), &[], vec![], vec![]);
        assert_eq!(d.breaks, vec![brk(1)]);
        assert!(d.jumps.is_empty());
        let d = j.tick(&g(0, 0), &[], vec![], vec![]);
        assert_eq!(d.jumps, vec![jump(2)]);
        assert!(d.breaks.is_empty());
    }
}
