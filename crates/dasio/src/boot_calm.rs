//! The service's wait for a calm Windows after a boot (dasio#28, owner
//! 23.11.2026: "pripadne by sa mal dasio aktivovat az ked je windows po
//! reboote v klude"). An SCM start within [`BOOT_WINDOW`] of the boot waits
//! before the device starts its media, until the PC has been quiet for
//! [`CALM_FOR_S`] seconds in a row (CPU busy and the disk queue low), at
//! most [`CAP_S`] seconds. A restart outside the window, `dasio run` and
//! the test clock never wait. The decisions are here, portable and tested
//! on every OS; [`windows`] reads the uptime, the CPU times and the disk
//! queue.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::{info, warn};

/// A service start this long after the boot (the system uptime) or later
/// is no boot start: the device starts at once.
pub const BOOT_WINDOW: Duration = Duration::from_secs(600);

/// One sample of the PC a second.
pub const SAMPLE: Duration = Duration::from_secs(1);

/// A second is calm with the PC's total CPU busy at or under this, in %
/// (every CPU, DPC and interrupt time included).
pub const CPU_CALM_PERCENT: f64 = 20.0;

/// ... and the disks' average queue over it at or under this (PDH
/// `\PhysicalDisk(_Total)\Avg. Disk Queue Length`).
pub const DISK_CALM_QUEUE: f64 = 0.5;

/// Calm seconds in a row before the media starts.
pub const CALM_FOR_S: u32 = 20;

/// The most seconds the device waits: then it starts anyway.
pub const CAP_S: u32 = 300;

/// A progress line every this many seconds of the wait.
pub const LOG_EVERY_S: u32 = 10;

/// Whether a service started `uptime` after the boot is a boot start.
pub fn is_boot(uptime: Duration) -> bool {
    uptime < BOOT_WINDOW
}

/// The PC's CPU times as `GetSystemTimes` gives them, 100 ns units summed
/// over every CPU: idle, kernel (idle included) and user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// The PC's CPU busy between two reads, in %; `None` when no time passed or
/// a counter went back.
pub fn busy_percent(before: CpuTimes, after: CpuTimes) -> Option<f64> {
    let idle = after.idle.checked_sub(before.idle)?;
    let kernel = after.kernel.checked_sub(before.kernel)?;
    let user = after.user.checked_sub(before.user)?;
    let total = kernel.checked_add(user)?;
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle);
    Some(busy as f64 * 100.0 / total as f64)
}

/// One second's look at the PC; `None` for what could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    pub cpu_percent: Option<f64>,
    pub disk_queue: Option<f64>,
}

/// A calm second: the CPU read and low, the disk queue low or unread (a
/// PC whose disk counters cannot be read is judged by its CPU alone).
pub fn is_calm(s: Sample) -> bool {
    s.cpu_percent.is_some_and(|c| c <= CPU_CALM_PERCENT)
        && s.disk_queue.is_none_or(|q| q <= DISK_CALM_QUEUE)
}

/// A figure for the progress line: `digits` decimals, `unread` without one.
pub fn shown(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "unread".to_string(), |v| format!("{v:.digits$}"))
}

/// What a sample means for the wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Go on; `log` when a progress line is due.
    Wait { log: bool },
    /// [`CALM_FOR_S`] calm seconds in a row.
    Calm,
    /// [`CAP_S`] seconds without that.
    Cap,
}

/// The wait's count of samples and of calm ones in a row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Calm {
    samples: u32,
    streak: u32,
}

impl Calm {
    /// Takes one second's sample.
    pub fn step(&mut self, s: Sample) -> Step {
        self.samples += 1;
        self.streak = if is_calm(s) { self.streak + 1 } else { 0 };
        if self.streak >= CALM_FOR_S {
            Step::Calm
        } else if self.samples >= CAP_S {
            Step::Cap
        } else {
            Step::Wait {
                log: self.samples.is_multiple_of(LOG_EVERY_S),
            }
        }
    }

    /// Seconds waited (one sample a second).
    pub fn waited_s(&self) -> u32 {
        self.samples
    }

    /// Calm seconds in a row so far.
    pub fn calm_s(&self) -> u32 {
        self.streak
    }
}

/// Why the device starts its media (or does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Windows was calm for [`CALM_FOR_S`] seconds.
    Calm,
    /// [`CAP_S`] seconds passed without that.
    Cap,
    /// The service started outside [`BOOT_WINDOW`].
    NotABoot,
    /// The test clock (CI): never waits.
    TestClock,
    /// A stop was asked during the wait: no start.
    Stopped,
}

impl Why {
    /// The start line's `reason`.
    pub fn name(self) -> &'static str {
        match self {
            Why::Calm => "calm",
            Why::Cap => "cap",
            Why::NotABoot => "not-a-boot",
            Why::TestClock => "test-clock",
            Why::Stopped => "stopped",
        }
    }
}

/// How the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub why: Why,
    pub waited_s: u32,
}

/// Whether a start `uptime` after the boot, on the test clock or not,
/// starts at once (and why), or waits (`None`).
pub fn at_once(uptime: Duration, test_clock: bool) -> Option<Why> {
    if test_clock {
        Some(Why::TestClock)
    } else if !is_boot(uptime) {
        Some(Why::NotABoot)
    } else {
        None
    }
}

/// Waits, a [`SAMPLE`] at a time (`pause`), until `read` gave
/// [`CALM_FOR_S`] calm samples in a row, [`CAP_S`] samples passed or `stop`
/// was asked. Logs a progress line every [`LOG_EVERY_S`] seconds.
pub fn wait(
    mut read: impl FnMut() -> Sample,
    stop: &AtomicBool,
    mut pause: impl FnMut(Duration),
) -> Outcome {
    let mut calm = Calm::default();
    for _ in 0..CAP_S {
        if stop.load(Ordering::Acquire) {
            return Outcome {
                why: Why::Stopped,
                waited_s: calm.waited_s(),
            };
        }
        pause(SAMPLE);
        let s = read();
        let why = match calm.step(s) {
            Step::Calm => Why::Calm,
            Step::Cap => Why::Cap,
            Step::Wait { log } => {
                if log {
                    info!(
                        waited_s = calm.waited_s(),
                        cpu_percent = %shown(s.cpu_percent, 1),
                        disk_queue = %shown(s.disk_queue, 2),
                        calm_s = calm.calm_s(),
                        "waiting for a calm Windows"
                    );
                }
                continue;
            }
        };
        return Outcome {
            why,
            waited_s: calm.waited_s(),
        };
    }
    Outcome {
        why: Why::Cap,
        waited_s: calm.waited_s(),
    }
}

/// Before the device starts (the service only): a start `uptime` after the
/// boot waits for a calm Windows with the reader `open` makes (only then),
/// unless on the test clock. Logs the wait's start and its end.
pub fn before_start<R: FnMut() -> Sample>(
    uptime: Duration,
    test_clock: bool,
    open: impl FnOnce() -> R,
    stop: &AtomicBool,
    pause: impl FnMut(Duration),
) -> Outcome {
    let outcome = match at_once(uptime, test_clock) {
        Some(why) => Outcome { why, waited_s: 0 },
        None => {
            info!(
                uptime_s = uptime.as_secs(),
                cpu_calm_percent = CPU_CALM_PERCENT,
                disk_calm_queue = DISK_CALM_QUEUE,
                calm_for_s = CALM_FOR_S,
                cap_s = CAP_S,
                "a boot: the device waits for a calm Windows before it starts its media"
            );
            wait(open(), stop, pause)
        }
    };
    let (warns, text) = end_line(outcome.why);
    if warns {
        warn!(
            reason = outcome.why.name(),
            waited_s = outcome.waited_s,
            service_start_uptime_s = uptime.as_secs(),
            "{text}"
        );
    } else {
        info!(
            reason = outcome.why.name(),
            waited_s = outcome.waited_s,
            service_start_uptime_s = uptime.as_secs(),
            "{text}"
        );
    }
    outcome
}

/// The wait's end line for `why`: whether it warns, and its text.
pub fn end_line(why: Why) -> (bool, &'static str) {
    match why {
        Why::Stopped => (
            false,
            "a stop was asked while waiting for a calm Windows: the device does not start",
        ),
        Why::Cap => (
            true,
            "Windows did not calm within the cap: the device starts its media anyway",
        ),
        Why::Calm | Why::NotABoot | Why::TestClock => (
            false,
            "the wait for a calm Windows is over: the device starts its media",
        ),
    }
}

#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests;
