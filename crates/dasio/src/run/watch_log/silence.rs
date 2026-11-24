//! How often a watched channel's "nothing checked" line is logged (dasio#207).
//! A channel with no tone or no program audio is silent in every 10 s window;
//! one line per window and channel filled the log (~15 MB a day, the log keeps
//! 4 x 10 MB). The first silent window of a run of them logs, then every
//! `EVERY`-th one (10 min).

/// A silent window is logged at the 1st, then at every `EVERY`-th after it.
const EVERY: u32 = 60;

/// The length of one summary window, seconds.
const WINDOW_S: u32 = 10;

/// Whether the `n`-th silent window in a row (1-based) is logged: 1, 61, 121, ...
pub(super) fn silent_due(n: u32) -> bool {
    n % EVERY == 1
}

/// Each channel's count of silent windows in a row.
#[derive(Default)]
pub(super) struct Silence {
    runs: Vec<u32>,
}

impl Silence {
    /// One channel's window ended: `checked` = it judged something. Returns
    /// the seconds the channel has been silent when its `nothing checked`
    /// line is due, else `None`. A checked window restarts the count.
    pub(super) fn note(&mut self, channel: usize, checked: bool) -> Option<u32> {
        if self.runs.len() <= channel {
            self.runs.resize(channel + 1, 0);
        }
        let run = &mut self.runs[channel];
        if checked {
            *run = 0;
            return None;
        }
        *run = run.saturating_add(1);
        silent_due(*run).then(|| run.saturating_mul(WINDOW_S))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_at_the_first_and_every_sixtieth_after() {
        assert!(!silent_due(0));
        assert!(silent_due(1));
        assert!(!silent_due(2));
        assert!(!silent_due(59));
        assert!(!silent_due(60));
        assert!(silent_due(61));
        assert!(!silent_due(62));
        assert!(!silent_due(120));
        assert!(silent_due(121));
        assert!(silent_due(601));
    }

    #[test]
    fn silent_seconds_count_the_windows_in_a_row() {
        let mut s = Silence::default();
        assert_eq!(s.note(0, false), Some(10));
        for _ in 2..=60 {
            assert_eq!(s.note(0, false), None);
        }
        assert_eq!(s.note(0, false), Some(610));
        assert_eq!(s.note(0, false), None);
    }

    #[test]
    fn a_checked_window_restarts_the_count() {
        let mut s = Silence::default();
        assert_eq!(s.note(0, false), Some(10));
        assert_eq!(s.note(0, false), None);
        assert_eq!(s.note(0, true), None);
        assert_eq!(s.note(0, false), Some(10));
        assert_eq!(s.note(0, false), None);
    }

    #[test]
    fn channels_count_apart() {
        let mut s = Silence::default();
        assert_eq!(s.note(1, false), Some(10));
        assert_eq!(s.note(1, false), None);
        // Channel 0 is new: its own first window is due.
        assert_eq!(s.note(0, false), Some(10));
        // Channel 1 checked: channel 0's count stays.
        assert_eq!(s.note(1, true), None);
        assert_eq!(s.note(0, false), None);
        assert_eq!(s.note(1, false), Some(10));
    }
}
