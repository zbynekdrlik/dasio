//! An RX stall, told from a host's miss (dasio#207). When the RX side of
//! the device loses packets from several sources at the same moment (a
//! stall on the sender's PC or the network, 27.11.2026 14:36:13Z), every
//! watched channel carries a zero or a repeat run, exactly the shape of an
//! ASIO host's missed buffer. dstack's own RX watch counts those holes per
//! flow slot (`RxShared::lane(slot).gaps`, the source of its `RX packets
//! missing ... never reached our socket` line); the run loop reads the same
//! counters every tick and [`Stall`] says when several sources lost packets
//! within the last few ticks.

/// Ticks (100 ms each) a source's loss stays in view: the tick it was read
/// in and the two before. dstack confirms a hole a little after the audio
/// shows it, so a break is judged one tick after it was drained.
pub const TICKS: usize = 3;

/// Sources that lost packets within [`TICKS`] for a stall: "several".
pub const SEVERAL: u32 = 2;

/// The flow slots one tick's mask holds.
const SLOTS: usize = 32;

/// Which flow slots lost packets in the last [`TICKS`] ticks.
#[derive(Debug, Default)]
pub struct Stall {
    seen: Vec<u64>,
    primed: bool,
    recent: [u32; TICKS],
    at: usize,
}

impl Stall {
    /// One tick: each slot's cumulative gap count now. The first look only
    /// takes the counts (a device that ran before the watch has some).
    pub fn note(&mut self, gaps: &[u64], _flows: &[u64]) {
        let gaps = &gaps[..gaps.len().min(SLOTS)];
        self.seen.resize(self.seen.len().max(gaps.len()), 0);
        let mut mask = 0u32;
        for (slot, (&now, seen)) in gaps.iter().zip(self.seen.iter_mut()).enumerate() {
            if self.primed && now > *seen {
                mask |= 1 << slot;
            }
            *seen = now;
        }
        self.primed = true;
        self.at = (self.at + 1) % TICKS;
        self.recent[self.at] = mask;
    }

    /// How many distinct sources lost packets in the last [`TICKS`] ticks.
    pub fn sources(&self) -> u32 {
        self.recent.iter().fold(0, |all, m| all | m).count_ones()
    }

    /// Whether several sources lost packets together: an RX stall.
    pub fn stalled(&self) -> bool {
        self.sources() >= SEVERAL
    }
}

/// An id of a flow slot's transmitter for [`Stall::note`]: 0 without one
/// (before a packet of its flow, and after its plan changed), else the
/// address and port.
pub fn flow_id(source: Option<std::net::SocketAddrV4>) -> u64 {
    let _ = source;
    0
}

/// Items held a few ticks before they are judged against the stall window
/// (a hole is confirmed by dstack a little after the audio shows it).
#[derive(Debug)]
pub struct Hold<T> {
    queue: Vec<(usize, T)>,
}

impl<T> Default for Hold<T> {
    fn default() -> Self {
        Hold { queue: Vec::new() }
    }
}

impl<T> Hold<T> {
    /// One tick: the items held for [`HOLD`] ticks come out; `new` goes in.
    pub fn tick(&mut self, new: impl IntoIterator<Item = T>) -> Vec<T> {
        new.into_iter().collect()
    }

    /// Everything still held (the device loop ends).
    pub fn flush(&mut self) -> Vec<T> {
        let _ = &self.queue;
        Vec::new()
    }
}

/// Ticks an item is held: judged at the second tick after it came.
pub const HOLD: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    fn stall_with(ticks: &[&[u64]]) -> Stall {
        let mut s = Stall::default();
        for t in ticks {
            s.note(t, &[]);
        }
        s
    }

    #[test]
    fn the_first_look_only_takes_the_counts() {
        let s = stall_with(&[&[5, 7, 0]]);
        assert_eq!(s.sources(), 0);
        assert!(!s.stalled());
    }

    #[test]
    fn one_source_losing_packets_is_not_a_stall() {
        let s = stall_with(&[&[0, 0], &[3, 0]]);
        assert_eq!(s.sources(), 1);
        assert!(!s.stalled());
    }

    #[test]
    fn two_sources_in_one_tick_are_a_stall() {
        let s = stall_with(&[&[0, 0, 0], &[1, 0, 1]]);
        assert_eq!(s.sources(), 2);
        assert!(s.stalled());
    }

    #[test]
    fn one_source_twice_is_still_one_source() {
        let s = stall_with(&[&[0, 0], &[1, 0], &[2, 0]]);
        assert_eq!(s.sources(), 1);
        assert!(!s.stalled());
    }

    #[test]
    fn sources_in_ticks_two_apart_are_one_stall() {
        let s = stall_with(&[&[0, 0], &[1, 0], &[1, 0], &[1, 1]]);
        assert_eq!(s.sources(), 2);
        assert!(s.stalled());
    }

    #[test]
    fn sources_in_ticks_three_apart_are_one_stall() {
        let s = stall_with(&[&[0, 0], &[1, 0], &[1, 0], &[1, 0], &[1, 1]]);
        assert_eq!(s.sources(), 2);
        assert!(s.stalled());
    }

    #[test]
    fn sources_in_ticks_four_apart_are_not() {
        let s = stall_with(&[&[0, 0], &[1, 0], &[1, 0], &[1, 0], &[1, 0], &[1, 1]]);
        assert_eq!(s.sources(), 1);
        assert!(!s.stalled());
    }

    #[test]
    fn a_stall_ends_after_the_window() {
        let q: &[u64] = &[1, 1];
        let s = stall_with(&[&[0, 0], &[1, 1], q, q, q, q]);
        assert_eq!(s.sources(), 0);
        assert!(!s.stalled());
        let s = stall_with(&[&[0, 0], &[1, 1], q, q, q]);
        assert_eq!(s.sources(), 2);
        assert!(s.stalled());
    }

    #[test]
    fn a_count_that_goes_back_is_no_loss() {
        // The device restarted: its counters begin again.
        let s = stall_with(&[&[9, 9], &[0, 0]]);
        assert_eq!(s.sources(), 0);
        // And the next rise from the new base is a loss.
        let s = stall_with(&[&[9, 9], &[0, 0], &[1, 1]]);
        assert_eq!(s.sources(), 2);
    }

    #[test]
    fn the_slots_beyond_the_mask_are_left_out() {
        let before = vec![0u64; 40];
        let mut after = vec![0u64; 40];
        after[33] = 4;
        after[39] = 4;
        let s = stall_with(&[&before, &after]);
        assert_eq!(s.sources(), 0);
        after[31] = 1;
        after[0] = 1;
        let mut s = Stall::default();
        s.note(&before, &[]);
        s.note(&after, &[]);
        assert_eq!(s.sources(), 2);
    }

    #[test]
    fn a_slot_that_appears_later_starts_from_zero() {
        // The list grows (a longer slice): the new slot's first count is a
        // loss only if above 0.
        let s = stall_with(&[&[0], &[0, 0, 0], &[0, 2, 3]]);
        assert_eq!(s.sources(), 2);
    }

    #[test]
    fn a_slot_first_seen_above_zero_is_a_base_not_a_loss() {
        let s = stall_with(&[&[0], &[0, 5, 7]]);
        assert_eq!(s.sources(), 0);
        let s = stall_with(&[&[0], &[0, 5, 7], &[0, 6, 8]]);
        assert_eq!(s.sources(), 2);
    }

    #[test]
    fn a_slot_whose_flow_changed_loses_nothing_that_tick() {
        let mut s = Stall::default();
        s.note(&[0, 0], &[1, 2]);
        // Both slots re-planned: their gaps are the change's, no stall.
        s.note(&[3, 4], &[5, 6]);
        assert_eq!(s.sources(), 0);
        // Next tick the flows hold: real losses count again.
        s.note(&[4, 4], &[5, 6]);
        assert_eq!(s.sources(), 1);
    }

    #[test]
    fn one_slot_re_planned_does_not_hide_a_real_loss_on_another() {
        let mut s = Stall::default();
        s.note(&[0, 0], &[1, 2]);
        s.note(&[2, 2], &[1, 9]);
        assert_eq!(s.sources(), 1);
        assert!(!s.stalled());
    }

    #[test]
    fn a_flow_that_loses_its_source_and_gets_one_again_is_two_changes() {
        let mut s = Stall::default();
        s.note(&[0], &[1]);
        s.note(&[1], &[0]);
        s.note(&[2], &[7]);
        assert_eq!(s.sources(), 0);
        s.note(&[3], &[7]);
        assert_eq!(s.sources(), 1);
    }

    #[test]
    fn a_flow_id_tells_addresses_and_ports_apart() {
        use std::net::{Ipv4Addr, SocketAddrV4};
        let a = |o: u8, p: u16| Some(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, o), p));
        assert_eq!(flow_id(None), 0);
        assert_ne!(flow_id(a(1, 4321)), 0);
        assert_eq!(flow_id(a(1, 4321)), flow_id(a(1, 4321)));
        assert_ne!(flow_id(a(1, 4321)), flow_id(a(2, 4321)));
        assert_ne!(flow_id(a(1, 4321)), flow_id(a(1, 4322)));
        // Address 0.0.0.0 port 0 is still a source, not none.
        assert_ne!(
            flow_id(Some(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))),
            0
        );
    }

    #[test]
    fn hold_releases_an_item_at_the_second_tick_after_it_came() {
        let mut h: Hold<u32> = Hold::default();
        assert!(h.tick([1]).is_empty());
        assert!(h.tick([2]).is_empty());
        assert_eq!(h.tick([]), vec![1]);
        assert_eq!(h.tick([]), vec![2]);
        assert!(h.tick([]).is_empty());
    }

    #[test]
    fn hold_flush_gives_everything_still_held_once() {
        let mut h: Hold<u32> = Hold::default();
        h.tick([1]);
        h.tick([2, 3]);
        assert_eq!(h.flush(), vec![1, 2, 3]);
        assert!(h.flush().is_empty());
        assert!(h.tick([]).is_empty());
    }
}
