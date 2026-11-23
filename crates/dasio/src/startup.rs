//! The start-up window's counts kept apart from the running ones
//! (dasio#28, owner 23.11.2026: "vypadky pri starte windows dasio az tak
//! nezaujimaju"). The first [`WINDOW`] after the device serves is its
//! start-up: the clock converges, the ASRC locks, the driver opens. Every
//! count dstack keeps is a total since the start (the runner's per driver
//! run), so a [`Split`] takes the totals each counts tick and puts what rose
//! since the last tick into the start-up counts or the running ones. The
//! counts lines and the status show the phase's own; the window's end logs
//! the start-up counts once. Pure, so tested everywhere; the run loops feed
//! it.

use std::time::Duration;

/// How long after the device serves its counts are the start-up's.
pub const WINDOW: Duration = Duration::from_secs(300);

/// Which counts a moment's rise belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Within [`WINDOW`] of the device's start.
    StartUp,
    /// After it.
    Running,
}

impl Phase {
    /// The counts line's and the status' `phase` value.
    pub fn name(self) -> &'static str {
        match self {
            Phase::StartUp => "start-up",
            Phase::Running => "running",
        }
    }
}

/// The phase at `since_start` after the device's start.
pub fn phase(since_start: Duration) -> Phase {
    if since_start < WINDOW {
        Phase::StartUp
    } else {
        Phase::Running
    }
}

/// What a counter rose from `last` to `now`. A counter that went down
/// restarted (a new driver run, a new shared memory section after a rate
/// change): all of its value is new, as in [`crate::app_status::LateWindow`].
fn rise(last: u64, now: u64) -> u64 {
    if now >= last { now - last } else { now }
}

/// What the status shows of the start-up window beside its counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part<const N: usize> {
    /// The window is still on: the counts shown are the start-up's own.
    pub in_window: bool,
    /// The start-up window's counts, once it ended (the counts shown are
    /// then the running ones).
    pub startup: [u64; N],
}

impl<const N: usize> Default for Part<N> {
    /// After the window, nothing counted in it: the status shows no
    /// start-up part.
    fn default() -> Self {
        Part {
            in_window: false,
            startup: [0; N],
        }
    }
}

/// `N` counters of one run, split into the start-up window's counts and
/// the running ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<const N: usize> {
    /// The totals at the last take.
    last: [u64; N],
    /// When the last take was, since the start.
    last_at: Duration,
    startup: [u64; N],
    running: [u64; N],
}

impl<const N: usize> Default for Split<N> {
    /// Nothing taken yet: the first take counts from 0, the start.
    fn default() -> Self {
        Split {
            last: [0; N],
            last_at: Duration::ZERO,
            startup: [0; N],
            running: [0; N],
        }
    }
}

impl<const N: usize> Split<N> {
    /// Takes the totals `now` at `since_start`: what rose since the last
    /// take goes to the phase that interval began in (the start-up window
    /// keeps its own last tick). Returns the start-up counts once, at the
    /// take that ends the window.
    pub fn take(&mut self, since_start: Duration, now: [u64; N]) -> Option<[u64; N]> {
        let was = phase(self.last_at);
        let into = match was {
            Phase::StartUp => &mut self.startup,
            Phase::Running => &mut self.running,
        };
        for ((count, &last), &total) in into.iter_mut().zip(&self.last).zip(&now) {
            *count = count.saturating_add(rise(last, total));
        }
        self.last = now;
        self.last_at = since_start;
        (was == Phase::StartUp && phase(since_start) == Phase::Running).then_some(self.startup)
    }

    /// The phase the counts shown now belong to.
    pub fn phase(&self) -> Phase {
        phase(self.last_at)
    }

    /// The counts to show with the totals `now` (live, not taken): the
    /// phase's counts and what rose since the last take.
    pub fn shown(&self, now: [u64; N]) -> [u64; N] {
        let counts = match self.phase() {
            Phase::StartUp => &self.startup,
            Phase::Running => &self.running,
        };
        std::array::from_fn(|i| counts[i].saturating_add(rise(self.last[i], now[i])))
    }

    /// The status' start-up part now.
    pub fn part(&self) -> Part<N> {
        Part {
            in_window: self.phase() == Phase::StartUp,
            startup: self.startup,
        }
    }
}

#[cfg(test)]
mod tests;
