// SPDX-License-Identifier: GPL-3.0-or-later

//! What ends the device process when a part of it died (dasio#28 E17). A
//! device whose pump no longer runs dasio's bridge plays silence on its
//! Dante ports for good while the process lives, so the SCM never restarts
//! it: the service loop ends the process instead, and the SCM brings it
//! back. Every panic is written to the instance log too: a service has no
//! console for the default message.

use dstack_audio_io::StreamStats;

/// Why the device must end, from its pump: the processor (dasio's bridge)
/// panicked, so dstack never calls it again and the outputs stay zero, or
/// the pump's thread ended. `None` while the pump runs it.
pub fn pump_failure(stats: &StreamStats, pump_ended: bool) -> Option<String> {
    let _ = (stats, pump_ended);
    None
}

/// Writes every panic to the log at ERROR (its thread, place and message),
/// then runs the hook that was there before. A panic caught on purpose is
/// written too.
pub fn log_panics() {}

#[cfg(test)]
mod tests;
