// SPDX-License-Identifier: GPL-3.0-or-later

//! The device's counters, one row a minute, kept on the PC for
//! [`KEEP_DAYS`] days (dasio#28 A36). The instance log rotates in about a
//! day, so a stall found days later (dasio#22) still has its minute here.
//! Rows go to `counters-YYYY-MM-DD.csv` (the UTC date) in the instance's
//! folder, a header first in each new file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Days of rows kept: older `counters-*.csv` files go when a new day starts.
pub const KEEP_DAYS: u64 = 30;

/// The CSV header: the minute (UTC), then what changed in it.
pub const HEADER: &str = "minute_utc,late,driver_missed,stale,waited,pump_late,missed,overruns,rebootstraps,blocks,over_period,max_us,wake_max_us,process_max_us,attached";

/// The device's counters at one counts tick, cumulative since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub late: u64,
    pub driver_missed: u64,
    pub stale: u64,
    pub waited: u64,
    pub pump_late: u64,
    pub missed: u64,
    pub overruns: u64,
    pub rebootstraps: u64,
}

impl Totals {
    /// What grew since `base` (a counter that went back counts 0).
    fn since(&self, base: &Totals) -> Totals {
        Totals {
            late: self.late.saturating_sub(base.late),
            driver_missed: self.driver_missed.saturating_sub(base.driver_missed),
            stale: self.stale.saturating_sub(base.stale),
            waited: self.waited.saturating_sub(base.waited),
            pump_late: self.pump_late.saturating_sub(base.pump_late),
            missed: self.missed.saturating_sub(base.missed),
            overruns: self.overruns.saturating_sub(base.overruns),
            rebootstraps: self.rebootstraps.saturating_sub(base.rebootstraps),
        }
    }

    fn add(&mut self, d: &Totals) {
        self.late += d.late;
        self.driver_missed += d.driver_missed;
        self.stale += d.stale;
        self.waited += d.waited;
        self.pump_late += d.pump_late;
        self.missed += d.missed;
        self.overruns += d.overruns;
        self.rebootstraps += d.rebootstraps;
    }
}

/// The DAW turnaround of one tick's window, as the counts line logs it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    pub blocks: u64,
    pub over: u64,
    pub max_us: u64,
    pub wake_max_us: u64,
    pub process_max_us: u64,
}

/// One minute: the counters' growth in it, the turnaround's blocks and
/// periods over, its longest turnaround and parts, and whether a DAW was
/// attached at the minute's last tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Row {
    /// The minute's start, seconds since the Unix epoch.
    pub minute: u64,
    pub grown: Totals,
    pub window: Window,
    pub attached: bool,
}

/// How far before a tick its window's middle lies: half the counts tick
/// (10 s). A tick's growth and turnaround belong to the minute that holds
/// that middle, so a stall is at most 5 s off its minute.
pub const HALF_TICK_S: u64 = 5;

/// Folds the counts ticks into minutes.
#[derive(Debug, Default)]
pub struct MinuteAgg {
    current: Option<Row>,
    last: Option<Totals>,
}

impl MinuteAgg {
    /// The tick at `unix_s` (seconds since the epoch): `totals` as they
    /// stand, `window` the turnaround since the last tick. The tick counts
    /// in the minute of its window's middle ([`HALF_TICK_S`] before it).
    /// Returns the minute before when this tick is the first of a later
    /// one; a tick of an earlier minute (the clock stepped back) counts in
    /// the current one, so no minute comes twice. The first tick only sets
    /// the base (nothing grew yet).
    pub fn tick(
        &mut self,
        unix_s: u64,
        totals: Totals,
        window: Window,
        attached: bool,
    ) -> Option<Row> {
        let at = unix_s.saturating_sub(HALF_TICK_S);
        let minute = at - at % 60;
        let grown = self
            .last
            .map(|base| totals.since(&base))
            .unwrap_or_default();
        self.last = Some(totals);
        let done = if self.current.is_some_and(|row| minute > row.minute) {
            self.current.take()
        } else {
            None
        };
        let row = self.current.get_or_insert(Row {
            minute,
            ..Row::default()
        });
        row.grown.add(&grown);
        row.window.blocks += window.blocks;
        row.window.over += window.over;
        row.window.max_us = row.window.max_us.max(window.max_us);
        row.window.wake_max_us = row.window.wake_max_us.max(window.wake_max_us);
        row.window.process_max_us = row.window.process_max_us.max(window.process_max_us);
        row.attached = attached;
        done
    }
}

/// The civil (proleptic Gregorian) date of day `z` after 1970-01-01
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// `YYYY-MM-DD` (UTC) of `unix_s`.
pub fn date(unix_s: u64) -> String {
    let (y, m, d) = civil_from_days((unix_s / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DDTHH:MMZ` (UTC) of `unix_s`.
pub fn minute_utc(unix_s: u64) -> String {
    let s = unix_s % 86_400;
    format!("{}T{:02}:{:02}Z", date(unix_s), s / 3_600, s % 3_600 / 60)
}

/// Whether `s` has the shape `YYYY-MM-DD`.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// One row as a CSV line (no newline).
pub fn line(r: &Row) -> String {
    let g = &r.grown;
    let w = &r.window;
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        minute_utc(r.minute),
        g.late,
        g.driver_missed,
        g.stale,
        g.waited,
        g.pump_late,
        g.missed,
        g.overruns,
        g.rebootstraps,
        w.blocks,
        w.over,
        w.max_us,
        w.wake_max_us,
        w.process_max_us,
        u8::from(r.attached)
    )
}

/// The daily CSV files in one folder.
pub struct MinuteLog {
    dir: PathBuf,
    keep_days: u64,
    open: Option<(String, File)>,
}

impl MinuteLog {
    pub fn new(dir: &Path, keep_days: u64) -> MinuteLog {
        MinuteLog {
            dir: dir.to_path_buf(),
            keep_days,
            open: None,
        }
    }

    /// Appends `row` to its day's file: an empty file gets the header
    /// first. Opening another day's file then removes the files older than
    /// `keep_days` days before it (after the row is in, so a failed removal
    /// never loses the row).
    pub fn write(&mut self, row: &Row) -> io::Result<()> {
        let day = date(row.minute);
        let opened = self.open.as_ref().is_none_or(|(open, _)| *open != day);
        if opened {
            let path = self.dir.join(format!("counters-{day}.csv"));
            let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
            if file.metadata()?.len() == 0 {
                writeln!(file, "{HEADER}")?;
            }
            self.open = Some((day, file));
        }
        let (_, file) = self.open.as_mut().expect("opened above");
        writeln!(file, "{}", line(row))?;
        if opened {
            self.prune(row.minute)?;
        }
        Ok(())
    }

    /// Removes `counters-YYYY-MM-DD.csv` files dated before `keep_days`
    /// days ahead of `unix_s`; nothing else in the folder. Every old file
    /// is tried; the first failure is returned after.
    fn prune(&self, unix_s: u64) -> io::Result<()> {
        let oldest = date(unix_s.saturating_sub(self.keep_days * 86_400));
        let mut failed = Ok(());
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(day) = name
                .to_str()
                .and_then(|n| n.strip_prefix("counters-"))
                .and_then(|n| n.strip_suffix(".csv"))
            else {
                continue;
            };
            if is_date(day)
                && day < oldest.as_str()
                && let Err(e) = fs::remove_file(entry.path())
                && failed.is_ok()
            {
                failed = Err(e);
            }
        }
        failed
    }
}

#[cfg(test)]
mod tests;
