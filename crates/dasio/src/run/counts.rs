//! The DAW kind's counts line, its counts split by phase (dasio#28, owner
//! 23.11.2026; the Windows glue over the portable [`crate::startup`]): the
//! start-up window's while it is on, the running ones after it. A DAW that
//! attaches opens a new window (owner 21.11.2026: misses count only from
//! some minutes after a DAW's start). The window's end logs the start-up
//! counts once.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dstack::stream::PumpView;
use shm::HostUse;
use tracing::info;

use super::serve::Current;
use crate::pump_loss::Lost;
use crate::startup::{Split, WINDOW};

/// The split counts of a DAW instance: the header's four (late,
/// driver_missed, stale, waited; the status shows them too, so its read
/// shares them) and the pump's, the line's only.
pub(super) struct Counts {
    header: Arc<Mutex<Split<4>>>,
    pump: Split<9>,
    /// A DAW was attached at the last tick.
    attached: bool,
}

impl Counts {
    pub(super) fn new() -> Counts {
        Counts {
            header: Arc::new(Mutex::new(Split::default())),
            pump: Split::default(),
            attached: false,
        }
    }

    /// The header's split, for the status read ([`super::serve::app_status`]).
    pub(super) fn header(&self) -> Arc<Mutex<Split<4>>> {
        Arc::clone(&self.header)
    }

    /// One counts tick, `since` the device serves: the serving now
    /// (`current`, its converter's dry frames `converter_dry`), the pump's
    /// counts `v` and its lost time `lost`. Logs the counts line.
    pub(super) fn tick(
        &mut self,
        since: Duration,
        current: &Current,
        converter_dry: u64,
        v: &PumpView,
        lost: Lost,
    ) {
        let h = current.shared.header();
        let attached = h.host_attached.load(Ordering::Relaxed) != 0;
        let (resets_asked, latencies_changed) = h.messages();
        let header_totals = [
            h.late.load(Ordering::Relaxed),
            h.missed.load(Ordering::Relaxed),
            current.stale.load(Ordering::Relaxed),
            current.waited.load(Ordering::Relaxed),
        ];
        let pump_totals = [
            v.late,
            v.missed,
            v.overruns,
            v.rebootstraps,
            lost.late_us,
            lost.device_us,
            u64::from(resets_asked),
            u64::from(latencies_changed),
            converter_dry,
        ];
        let mut header = self.header.lock().unwrap_or_else(PoisonError::into_inner);
        // A DAW that attached since the last tick: its start's counts
        // (this tick's included) are start-up counts too.
        if attached && !self.attached {
            header.restart_window();
            self.pump.restart_window();
            info!(
                window_s = WINDOW.as_secs(),
                "a DAW attached: its start's counts are start-up counts for a new window"
            );
        }
        self.attached = attached;
        let header_tick = header.tick(since, header_totals);
        drop(header);
        let pump_tick = self.pump.tick(since, pump_totals);
        if let (Some(header_startup), Some(pump_startup)) = (header_tick.ended, pump_tick.ended) {
            startup_ended(header_startup, pump_startup);
        }
        let [late, driver_missed, stale, waited] = header_tick.shown;
        let [
            pump_late,
            missed,
            overruns,
            rebootstraps,
            pump_lost_late_us,
            pump_lost_device_us,
            resets_asked,
            latencies_changed,
            converter_dry,
        ] = pump_tick.shown;
        info!(
            phase = header_tick.phase.name(),
            seq = h.seq.load(Ordering::Relaxed),
            ack = h.ack.load(Ordering::Relaxed),
            late,
            driver_missed,
            stale,
            waited,
            attached = h.host_attached.load(Ordering::Relaxed),
            blocks = v.blocks,
            pump_late,
            missed,
            overruns,
            rebootstraps,
            max_process_us = v.max_process_us,
            // The pump's lost time by cause (dasio#114).
            pump_lost_late_us,
            pump_lost_device_us,
            resets_asked,
            latencies_changed,
            // How the DAW drives the driver (dasio#28 E4); 0 from a
            // driver before 0.1.36.
            output_ready = u8::from(h.uses(HostUse::OutputReady)),
            time_info = u8::from(h.uses(HostUse::TimeInfo)),
            latency_reads = h.latency_reads.load(Ordering::Relaxed),
            // The app's rate and its conversion (dasio#107): frames
            // played as silence by the converter's dry TX FIFO.
            app_rate = current.plan.app_rate,
            converter_dry,
            "counts"
        );
    }
}

/// A start-up window's end: its counts (all windows' so far, the header's
/// and the pump's, in the split order), apart from the running ones the
/// counts lines carry from here.
fn startup_ended(header: [u64; 4], pump: [u64; 9]) {
    let [late, driver_missed, stale, waited] = header;
    let [
        pump_late,
        missed,
        overruns,
        rebootstraps,
        pump_lost_late_us,
        pump_lost_device_us,
        resets_asked,
        latencies_changed,
        converter_dry,
    ] = pump;
    info!(
        window_s = WINDOW.as_secs(),
        late,
        driver_missed,
        stale,
        waited,
        pump_late,
        missed,
        overruns,
        rebootstraps,
        pump_lost_late_us,
        pump_lost_device_us,
        resets_asked,
        latencies_changed,
        converter_dry,
        "the start-up window ended: its counts are kept apart, the counts lines count the running ones from here"
    );
}
