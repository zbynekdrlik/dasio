//! The bridge instance's look at its to-ASIO pull (dasio#120, dstack issue
//! 307 step 2, dstack v0.6.155): the driver's buffer switch drains the
//! flow sockets and reads the RX rings by position into the to-ASIO FIFO
//! itself (`dstack_asio::bridge::Bridge::pull_to_asio`), so a late pump
//! wake no longer runs that FIFO dry. The instance's own loop looks at the
//! pull every [`KEEP_EVERY`] (`Bridge::keep_pulled`): while no driver side
//! pulls it drains the flow itself, so the flow stays heard.
//!
//! The decisions here (the period, what a look logs) are portable and
//! tested on Linux; `run_bridge.rs` (Windows glue) only calls them.

use std::time::Duration;

use dlink::bridge::pull::Look;

/// The period of the look at the pull, on the instance's own timer (never
/// the pump, never the driver's callback), as dstack v0.6.155's Consumers
/// line asks. dstack's rule for `Bridge::keep_pulled`: longer than the
/// driver's buffer period (under 150 ms at the default target), or a
/// running driver reads as stopped between two switches and the idle
/// drain restarts its read; and well inside a flow's 2 s keepalive, so a
/// flow no driver pulls stays heard. dlisten looks every 250 ms too. The
/// run loop passes every 100 ms, so a look comes every 250-350 ms.
pub const KEEP_EVERY: Duration = Duration::from_millis(250);

/// One line a look gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// A pull panicked; its poisoned lock was cleared and the switches
    /// pull on.
    Panicked,
    /// No driver side pulls (any more, when `after_pulls`: a driver that
    /// pulled stopped; else none pulled yet, as at the start before the
    /// driver's first open): the look drains the flow itself.
    Idle { after_pulls: bool },
    /// A driver side pulls again.
    Pulls,
    /// The pull started again at now this many times since the look
    /// before (a gap past the FIFO or a clock step).
    Restarted(u64),
}

impl Note {
    /// Whether the line is a WARN (else INFO): not the driver pulling
    /// (again), nor the start's idle before any pull (a driver whose first
    /// open takes longer than a look, e.g. SoundGrid's).
    pub fn warns(self) -> bool {
        !matches!(self, Note::Pulls | Note::Idle { after_pulls: false })
    }

    /// The line's message.
    pub fn text(self) -> &'static str {
        match self {
            Note::Panicked => "a to-ASIO pull panicked: its lock was cleared, the driver pulls on",
            Note::Idle { .. } => {
                "no ASIO driver side pulls the to-ASIO audio: the instance drains the flow itself"
            }
            Note::Pulls => "the ASIO driver pulls the to-ASIO audio again",
            Note::Restarted(_) => {
                "the to-ASIO pull started again at now (a gap past the FIFO or a clock step)"
            }
        }
    }
}

/// The lines for one look (`Bridge::keep_pulled`'s `(look, poisoned)`,
/// `pulls` the pull's count so far), in order: a cleared poison, the
/// change of who pulls (both ways, once at the change, never again while
/// it stays), the restarts' rise.
pub fn notes(look: Look, poisoned: bool, pulls: u64) -> Vec<Note> {
    let mut out = Vec::new();
    if poisoned {
        out.push(Note::Panicked);
    }
    if look.changed {
        out.push(if look.idle {
            Note::Idle {
                after_pulls: pulls > 0,
            }
        } else {
            Note::Pulls
        });
    }
    if look.restarts_rose > 0 {
        out.push(Note::Restarted(look.restarts_rose));
    }
    out
}

#[cfg(test)]
mod tests;
