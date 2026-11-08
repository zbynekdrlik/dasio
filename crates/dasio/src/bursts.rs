//! Bursts of blocks the DAW missed (dasio#1, 8.11.2026): the run loop's
//! 100 ms ticks turn the shared memory's `missed` and `late` counters into
//! one line when a burst starts and one when it ends, so a miss is placed
//! to ~0.1 s (the counts line gives only 10 s totals). A burst ends only
//! after [`QUIET_TICKS`] quiet ticks, so misses that come and go merge into
//! one burst instead of a line pair each; a DAW that hangs (a rise every
//! tick) is two lines too. Pure, so tested everywhere.

/// Quiet ticks in a row that end a burst (1 s at the run loop's 100 ms).
pub const QUIET_TICKS: u32 = 10;

/// What a tick found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Blocks were missed or late since the last tick, with no burst open:
    /// the counts since then.
    Start { missed: u64, late: u64 },
    /// A burst is over ([`QUIET_TICKS`] quiet ticks, or the counters
    /// restarted): its totals, and ms from its first rise to its last.
    End { missed: u64, late: u64, ms: u64 },
}

/// A burst in progress.
#[derive(Debug, Clone, Copy)]
struct Open {
    /// The tick that started it and the last one that saw a rise (ms).
    since: u64,
    last_rise: u64,
    /// The counters before it.
    missed0: u64,
    late0: u64,
    /// Quiet ticks since its last rise.
    quiet: u32,
}

/// The counters at the last tick and the burst in progress.
#[derive(Debug, Default)]
pub struct Bursts {
    last: Option<(u64, u64)>,
    open: Option<Open>,
}

impl Bursts {
    /// The counters `missed` and `late` at `now_ms` (any steady clock).
    pub fn tick(&mut self, now_ms: u64, missed: u64, late: u64) -> Option<Event> {
        let (m0, l0) = self.last.replace((missed, late))?;
        if missed < m0 || late < l0 {
            // Counters that went down: a new shared memory, not a burst.
            // A burst still open ends with what it counted until then.
            return self.open.take().map(|o| end(&o, m0, l0));
        }
        let rose = missed > m0 || late > l0;
        match (rose, self.open.as_mut()) {
            (true, None) => {
                self.open = Some(Open {
                    since: now_ms,
                    last_rise: now_ms,
                    missed0: m0,
                    late0: l0,
                    quiet: 0,
                });
                Some(Event::Start {
                    missed: missed - m0,
                    late: late - l0,
                })
            }
            (true, Some(o)) => {
                o.last_rise = now_ms;
                o.quiet = 0;
                None
            }
            (false, Some(o)) => {
                o.quiet += 1;
                if o.quiet < QUIET_TICKS {
                    return None;
                }
                self.open.take().map(|o| end(&o, missed, late))
            }
            (false, None) => None,
        }
    }
}

/// The end of burst `o` with the counters at `missed` and `late`.
fn end(o: &Open, missed: u64, late: u64) -> Event {
    Event::End {
        missed: missed - o.missed0,
        late: late - o.late0,
        ms: o.last_rise - o.since,
    }
}

#[cfg(test)]
mod tests;
