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
    if stats.faulted {
        let why = stats.fault.as_deref().unwrap_or("no message");
        return Some(format!(
            "the bridge panicked in the pump ({why}): its outputs stay silent, so the device ends for the SCM to restart it"
        ));
    }
    if pump_ended {
        return Some(
            "the pump's thread ended: no block reaches the DAW, so the device ends for the SCM to restart it"
                .to_string(),
        );
    }
    None
}

/// Writes every panic to the log at ERROR (its thread, place and message),
/// then runs the hook that was there before. A panic caught on purpose is
/// written too.
pub fn log_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("(not text)");
        let place = info.location().map_or_else(
            || "unknown".to_string(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        let thread = std::thread::current();
        tracing::error!(
            thread = %thread.name().unwrap_or("unnamed"),
            place = %place,
            panic = %message,
            "a panic"
        );
        before(info);
    }));
}

#[cfg(test)]
mod tests;
