//! A bridge instance's counts split by phase (dasio#28, owner 23.11.2026;
//! the Windows glue over the portable [`crate::startup`]): the start-up
//! window's while it is on, the running ones after it, in its counts line
//! and its status. The window's end logs the start-up counts once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dstack::stream::PumpView;
use dstack_asio::Status;
use dstack_asio::state::LATE_BINS;
use tracing::info;

use super::lock;
use crate::startup::{Part, Phase, Split, WINDOW};

/// The split counts the status shows too: the underruns to and from ASIO
/// and the resets (in [`crate::app_status::BRIDGE_STARTUP`] order) and the
/// driver callbacks' lateness bins; one lock for the status read and the
/// counts tick.
#[derive(Default)]
pub(super) struct StatusSplit {
    counts: Split<3>,
    bins: Split<LATE_BINS>,
}

/// What the status shows of them now.
pub(super) struct StatusShown {
    pub to_asio_underruns: u64,
    pub from_asio_underruns: u64,
    pub resets: u64,
    pub late_bins: [u64; LATE_BINS],
    pub startup: Part<3>,
}

impl StatusSplit {
    /// The status' counts with the live runner status `s` and players'
    /// underruns (not taken: the counts tick takes them).
    pub(super) fn shown(
        &self,
        s: &Status,
        to_asio_underruns: u64,
        from_asio_underruns: u64,
    ) -> StatusShown {
        let [to_asio_underruns, from_asio_underruns, resets] =
            self.counts
                .shown([to_asio_underruns, from_asio_underruns, s.resets]);
        StatusShown {
            to_asio_underruns,
            from_asio_underruns,
            resets,
            late_bins: self.bins.shown(s.late_bins),
            startup: self.counts.part(),
        }
    }
}

/// The players' totals a counts tick takes.
pub(super) struct Players {
    pub to_asio_underruns: u64,
    pub from_asio_underruns: u64,
    /// The to-ASIO FIFO's overflows and its pull's restarts and idle
    /// drains.
    pub to_asio_overflows: u64,
    pub to_asio_pull_restarts: u64,
    pub to_asio_idle_drains: u64,
}

/// One counts tick's counts, the phase's own.
pub(super) struct Shown {
    pub phase: Phase,
    pub to_asio_underruns: u64,
    pub from_asio_underruns: u64,
    pub resets: u64,
    pub late_bins: [u64; LATE_BINS],
    pub late_callbacks: u64,
    pub fast_callbacks: u64,
    pub overloads: u64,
    pub faults: u64,
    pub clock_waits: u64,
    pub resyncs: u64,
    pub to_asio_overflows: u64,
    pub to_asio_pull_restarts: u64,
    pub to_asio_idle_drains: u64,
    pub pump_late: u64,
    pub missed: u64,
    pub overruns: u64,
}

/// The bridge's split counts: the status' (shared with its read) and the
/// line's own.
pub(super) struct Counts {
    status: Arc<Mutex<StatusSplit>>,
    line: Split<12>,
}

impl Counts {
    pub(super) fn new() -> Counts {
        Counts {
            status: Arc::new(Mutex::new(StatusSplit::default())),
            line: Split::default(),
        }
    }

    /// The status' split, for the status read.
    pub(super) fn status(&self) -> Arc<Mutex<StatusSplit>> {
        Arc::clone(&self.status)
    }

    /// One counts tick, `since` the device serves: the runner's status
    /// `s`, the players' totals `p` and the pump's counts `v`. Logs the
    /// window's end.
    pub(super) fn tick(&mut self, since: Duration, s: &Status, p: &Players, v: &PumpView) -> Shown {
        let mut split = lock(&self.status);
        let status = split.counts.tick(
            since,
            [p.to_asio_underruns, p.from_asio_underruns, s.resets],
        );
        let bins = split.bins.tick(since, s.late_bins);
        drop(split);
        let line = self.line.tick(
            since,
            [
                s.late_callbacks,
                s.fast_callbacks,
                s.overloads,
                s.faults,
                s.clock_waits,
                s.resyncs,
                p.to_asio_overflows,
                p.to_asio_pull_restarts,
                p.to_asio_idle_drains,
                v.late,
                v.missed,
                v.overruns,
            ],
        );
        if let (Some(status), Some(bins), Some(line)) = (status.ended, bins.ended, line.ended) {
            startup_ended(status, bins, line);
        }
        let [to_asio_underruns, from_asio_underruns, resets] = status.shown;
        let [
            late_callbacks,
            fast_callbacks,
            overloads,
            faults,
            clock_waits,
            resyncs,
            to_asio_overflows,
            to_asio_pull_restarts,
            to_asio_idle_drains,
            pump_late,
            missed,
            overruns,
        ] = line.shown;
        Shown {
            phase: status.phase,
            to_asio_underruns,
            from_asio_underruns,
            resets,
            late_bins: bins.shown,
            late_callbacks,
            fast_callbacks,
            overloads,
            faults,
            clock_waits,
            resyncs,
            to_asio_overflows,
            to_asio_pull_restarts,
            to_asio_idle_drains,
            pump_late,
            missed,
            overruns,
        }
    }
}

/// The start-up window's end: its counts (the status', the lateness bins
/// and the line's, in the split order), apart from the running ones the
/// counts lines carry from here.
fn startup_ended(status: [u64; 3], late_bins: [u64; LATE_BINS], line: [u64; 12]) {
    let [to_asio_underruns, from_asio_underruns, resets] = status;
    let [
        late_callbacks,
        fast_callbacks,
        overloads,
        faults,
        clock_waits,
        resyncs,
        to_asio_overflows,
        to_asio_pull_restarts,
        to_asio_idle_drains,
        pump_late,
        missed,
        overruns,
    ] = line;
    info!(
        window_s = WINDOW.as_secs(),
        to_asio_underruns,
        from_asio_underruns,
        resets,
        late_callbacks,
        late_bins = ?late_bins,
        fast_callbacks,
        overloads,
        faults,
        clock_waits,
        resyncs,
        to_asio_overflows,
        to_asio_pull_restarts,
        to_asio_idle_drains,
        pump_late,
        missed,
        overruns,
        "the start-up window ended: its counts are kept apart, the counts lines count the running ones from here"
    );
}
