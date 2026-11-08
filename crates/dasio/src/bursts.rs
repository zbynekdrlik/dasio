//! Bursts of blocks the DAW missed (dasio#1, 8.11.2026): the run loop's
//! 100 ms ticks turn the shared memory's `missed` and `late` counters into
//! one line when a burst starts and one when it ends, so a miss is placed
//! to ~0.1 s (the counts line gives only 10 s totals) and a hung DAW logs
//! two lines, not one per tick. Pure, so tested everywhere.

/// What a tick found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Blocks were missed or late since the last tick, after a quiet one:
    /// the counts since then.
    Start { missed: u64, late: u64 },
    /// The first quiet tick after a burst: its totals and length (ms from
    /// the tick that started it to this one).
    End { missed: u64, late: u64, ms: u64 },
}

/// The counters at the last tick and the burst in progress.
#[derive(Debug, Default)]
pub struct Bursts {
    last: Option<(u64, u64)>,
    /// The burst's start: its tick (ms) and the counters before it.
    open: Option<(u64, u64, u64)>,
}

impl Bursts {
    /// The counters `missed` and `late` at `now_ms` (any steady clock).
    pub fn tick(&mut self, now_ms: u64, missed: u64, late: u64) -> Option<Event> {
        let (m0, l0) = self.last.replace((missed, late))?;
        if missed < m0 || late < l0 {
            // Counters that went down: a new shared memory, not a burst.
            self.open = None;
            return None;
        }
        let rose = missed > m0 || late > l0;
        match (rose, self.open) {
            (true, None) => {
                self.open = Some((now_ms, m0, l0));
                Some(Event::Start {
                    missed: missed - m0,
                    late: late - l0,
                })
            }
            (false, Some((at, bm, bl))) => {
                self.open = None;
                Some(Event::End {
                    missed: missed - bm,
                    late: late - bl,
                    ms: now_ms.saturating_sub(at),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
