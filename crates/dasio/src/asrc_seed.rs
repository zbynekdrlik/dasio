//! A bridge instance's ASRC seed file (dasio#120, dstack issue 382,
//! dstack v0.6.172): the rates the two players' rate fits last locked on,
//! so the next run's correction starts there instead of at 0 ppm (a driver
//! 7.5 ppm off Dante drifted the FIFO's depth about 0.45 ms in the minute
//! before the fit locked, dsg 17.11.2026).
//!
//! dstack-asio owns the file's format and its whole-file write
//! ([`Seed::read`], [`Seed::write`]); dasio names the file and decides when
//! to write: [`path`] in the instance's folder (beside its `device.toml`),
//! one file per driver, read once at the start (before the driver's first
//! open) and written from the run loop on its own [`SAVE_EVERY`] timer,
//! only when [`Keeper::due`] says the seed changed. Pure, tested on every
//! OS; `run_bridge.rs` (Windows glue) calls it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dstack_asio::bridge::Seed;

/// How often the run loop looks for a new seed to write: a rate fit locks
/// 60 s after an open, so once a minute is enough (dstack-asio's doc).
pub const SAVE_EVERY: Duration = Duration::from_secs(60);

/// The seed file of the bridge to `driver` in the instance folder `dir`:
/// `asrc-seed-<driver>.toml`, the driver's name kept to letters, digits,
/// `-` and `_` (every other character `_`), so another driver's file is
/// never read for this one.
pub fn path(dir: &Path, driver: &str) -> PathBuf {
    let name: String = driver
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if name.is_empty() {
        "driver"
    } else {
        name.as_str()
    };
    dir.join(format!("asrc-seed-{name}.toml"))
}

/// What the start does with the file's read: the seed to start the ASRC
/// from (`Some` only for a readable seed), and a line for the log (`warn`
/// true for a file that could not be read or is no seed: the bridge starts
/// unseeded and the next lock writes over it).
pub fn at_start(read: &Result<Option<Seed>, String>) -> (Option<Seed>, bool, String) {
    match read {
        Ok(Some(s)) => (
            Some(*s),
            false,
            format!(
                "the ASRC starts from the last run's locked rates: {}",
                describe(s)
            ),
        ),
        Ok(None) => (
            None,
            false,
            "no ASRC seed yet: the rate fits start at 0 ppm and lock 60 s after the driver's open"
                .to_string(),
        ),
        Err(e) => (
            None,
            true,
            format!("the ASRC seed is not readable ({e}): the bridge starts unseeded"),
        ),
    }
}

/// A seed in one line: its rates and both players' ppm.
pub fn describe(s: &Seed) -> String {
    let ppm = |p: Option<f64>| p.map_or_else(|| "none".to_string(), |p| format!("{p:.3} ppm"));
    format!(
        "Dante {} Hz, driver {} Hz, to ASIO {}, from ASIO {}",
        s.dante_rate,
        s.driver_rate,
        ppm(s.to_asio_ppm),
        ppm(s.from_asio_ppm)
    )
}

/// When the run loop writes the seed: only a seed that differs from the
/// one in the file, and a failed write retried at the next look, its
/// error logged once until it changes or a write succeeds.
#[derive(Debug, Default)]
pub struct Keeper {
    /// The seed the file holds (read at the start or last written).
    last: Option<Seed>,
    /// The last write's error, already logged.
    failed: Option<String>,
}

impl Keeper {
    /// A keeper for a file holding `start` (the start's read; `None` for
    /// no file or one that was no seed).
    pub fn new(start: Option<Seed>) -> Keeper {
        Keeper {
            last: start,
            failed: None,
        }
    }

    /// The seed to write now: the bridge's current seed when it differs
    /// from the file's.
    pub fn due(&self, now: Option<Seed>) -> Option<Seed> {
        now.filter(|s| self.last != Some(*s))
    }

    /// Records the write of `seed`; returns the error to log (once per
    /// distinct error, until a write succeeds).
    pub fn wrote(&mut self, seed: Seed, result: Result<(), String>) -> Option<String> {
        match result {
            Ok(()) => {
                self.last = Some(seed);
                self.failed = None;
                None
            }
            Err(e) if self.failed.as_deref() == Some(e.as_str()) => None,
            Err(e) => {
                self.failed = Some(e.clone());
                Some(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;
