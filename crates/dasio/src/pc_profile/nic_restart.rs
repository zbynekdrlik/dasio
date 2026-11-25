//! A written Dante NIC value is not yet a running one (dasio#26).
//!
//! `apply` writes the NIC's advanced settings into the registry and never
//! restarts the adapter; the driver reads them when it starts. The check
//! reads the same registry, so right after an apply a value reads fine
//! while the running driver still holds the old one. Windows has no
//! general API for the running value of an advanced property (the
//! `MSFT_NetAdapter*` classes cover a few offloads only, and need a
//! process the bridge's in-process check never starts), so the check
//! compares two times instead: the last write of the NIC's registry keys
//! against the time the adapter last started.
//!
//! This part is portable and tested everywhere; reading the two times is
//! [`windows`](super::windows).

use super::{Problem, Profile, Reader, applies, on_nic, want_reg};

/// A key written this long (seconds) after the adapter started still
/// counts as written before it: a driver may write its own keys while it
/// starts.
pub const SLACK_SECS: u64 = 10;

/// Seconds between 1601-01-01 (a Windows FILETIME's zero) and 1970-01-01.
const FILETIME_TO_UNIX_SECS: u64 = 11_644_473_600;

/// The two times the decision compares, in seconds since 1970.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NicTimes {
    /// The later last-write time of the NIC's class key and device key.
    pub keys_written: u64,
    /// When the adapter's device last started (its last arrival).
    pub started: u64,
}

/// A FILETIME (100 ns ticks since 1601) in seconds since 1970; 0 for one
/// before 1970.
pub fn filetime_secs(ticks: u64) -> u64 {
    (ticks / 10_000_000).saturating_sub(FILETIME_TO_UNIX_SECS)
}

/// Whether the keys were written after the adapter started, past the
/// [`SLACK_SECS`] a start may take to write its own.
pub fn pending(t: &NicTimes) -> bool {
    t.keys_written > t.started.saturating_add(SLACK_SECS)
}

/// The one problem a written, not yet restarted NIC gives: only when an
/// entry on the NIC's keys applies and reads as the profile wants (a
/// mismatch is its own problem already), and the keys were written after
/// the adapter started. `Ok(None)` from the reader (NIC not found, no
/// times to read) says nothing: the missing-NIC problem covers it.
pub fn check(p: &Profile, r: &dyn Reader) -> Option<Problem> {
    let held = p.registry.iter().any(|e| {
        on_nic(&e.key)
            && applies(e, r)
            && want_reg(e)
                .is_some_and(|want| matches!(r.reg(&e.key, &e.name), Ok(Some(v)) if v == want))
    });
    if !held {
        return None;
    }
    let found = match r.nic_times() {
        Ok(None) => return None,
        Ok(Some(t)) if pending(&t) => format!(
            "written, NIC not restarted (the keys were written {} s after the adapter started; the running driver may still hold the old values)",
            t.keys_written - t.started
        ),
        Ok(Some(_)) => return None,
        Err(e) => {
            format!("unreadable ({e}): cannot tell whether the NIC restarted since the write")
        }
    };
    Some(Problem {
        what: "the Dante NIC's registry settings".into(),
        found,
        want: "a restart of the NIC after the write (dasio#26)".into(),
        why: "apply writes NIC values but never restarts the adapter".into(),
        must: false,
    })
}
