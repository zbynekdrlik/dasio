//! Services Windows turns back on by itself, held off by the service
//! (dasio#158). Windows' own servicing (WaaSMedic, SYSTEM) sets UsoSvc, the
//! Update Orchestrator, back to automatic start at boot (FOH, SCM event
//! 7040 on 8.11 and 15.11.2026), so a profile check alone only reports it
//! after the fact, and `apply` runs by hand. The service's profile thread
//! runs [`decide`] for each [`HELD`] service right before every check (the
//! first one after start, then every [`CHECK_EVERY_S`](super::CHECK_EVERY_S))
//! and the Windows side sets the start type back and stops it, through the
//! SCM, in process (no child process: the bridge kind runs it too), at the
//! thread's lowest priority. A reset after a check is undone at the next
//! one: UsoSvc can run up to 10 min after it.
//!
//! Windows Update stays usable: dasio's planned runs (dasio#109, `wu.ps1`)
//! use the Windows Update Agent API, served by `wuauserv` (manual start),
//! not by UsoSvc (mbc 13.11.2026: five updates installed with UsoSvc
//! disabled and stopped).
//!
//! This part is portable and tested everywhere; the reading and the fix are
//! [`windows`](self::windows) (Windows only).

#[cfg(windows)]
pub mod windows;

/// One service dasio holds at a start type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// The service's name (its key under `Services`).
    pub service: &'static str,
    /// The start type dasio holds it at (4 = disabled).
    pub start: u32,
}

/// The services Windows resets by itself. Each one's start type is the
/// baseline's `Services\<name>\Start` entry too (a test keeps them equal);
/// widen the list here, under review, never from a profile.
pub const HELD: &[Held] = &[Held {
    service: "UsoSvc",
    start: 4,
}];

/// What to do to a held service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fix {
    /// The start type found, when it must be set back.
    pub was: Option<u32>,
    /// It runs (any state but stopped), so it must be stopped.
    pub stop: bool,
}

/// What `held` needs: `start` is its `Start` value (`None`: the value is
/// not there, so the service is not installed and nothing is done),
/// `running` whether the SCM sees it in any state but stopped. `None` when
/// it is as it should be.
pub fn decide(held: &Held, start: Option<u32>, running: bool) -> Option<Fix> {
    let start = start?;
    let was = (start != held.start).then_some(start);
    let stop = running && held.start == DISABLED;
    (was.is_some() || stop).then_some(Fix { was, stop })
}

/// A service's `Start` value for "disabled".
pub const DISABLED: u32 = 4;

/// A start type as a word.
pub fn start_name(start: u32) -> &'static str {
    match start {
        0 => "boot",
        1 => "system",
        2 => "automatic",
        3 => "manual",
        4 => "disabled",
        _ => "unknown",
    }
}

/// The log line for `fix` done to `held`: `set` is what setting the start
/// type back gave, `stopped` what the stop gave (`Ok`, or `Err` naming why
/// not); each unused when `fix` does not ask for it.
pub fn describe(
    held: &Held,
    fix: &Fix,
    set: &Result<(), String>,
    stopped: &Result<(), String>,
) -> String {
    let mut parts = Vec::new();
    if let Some(was) = fix.was {
        let (want, want_name) = (held.start, start_name(held.start));
        parts.push(match set {
            Ok(()) => format!(
                "start was {was} ({}), set to {want} ({want_name})",
                start_name(was)
            ),
            Err(e) => format!(
                "start is {was} ({}), not set to {want} ({want_name}): {e}",
                start_name(was)
            ),
        });
    }
    if fix.stop {
        parts.push(match stopped {
            Ok(()) => "it was running: stopped".to_owned(),
            Err(e) => format!("it runs and did not stop: {e}"),
        });
    }
    format!(
        "{} held off (dasio#158): {}",
        held.service,
        parts.join("; ")
    )
}

/// The log line when `held` could not even be looked at (`why`).
pub fn not_held(held: &Held, why: &str) -> String {
    format!("{} not held (dasio#158): {why}", held.service)
}
