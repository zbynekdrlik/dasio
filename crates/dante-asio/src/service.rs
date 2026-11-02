//! `dante-asio service`: the device as a Windows service (one per instance,
//! started at boot, restarted by the SCM's recovery actions). The decisions
//! are here, portable and tested on every OS; [`windows`] holds the SCM glue.

use std::net::{IpAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The SCM's wait hint while the device stops: its own stop's 7 s and the
/// pump's end.
pub const STOP_HINT: Duration = Duration::from_secs(10);

/// Instance `instance`'s service name (`install.ps1` registers it).
pub fn name(instance: u32) -> String {
    format!("dante-asio-{instance}")
}

/// A control the SCM sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Stop,
    Shutdown,
    Interrogate,
    Other,
}

/// What the service does with a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Ask the device to stop (as `dante-asio stop` does) and report
    /// StopPending with [`STOP_HINT`].
    Stop,
    /// Report the current status.
    Report,
    /// Not accepted.
    NotImplemented,
}

pub fn answer(c: Control) -> Answer {
    match c {
        Control::Stop | Control::Shutdown => Answer::Stop,
        Control::Interrogate => Answer::Report,
        Control::Other => Answer::NotImplemented,
    }
}

/// The service's exit code for the SCM: 0 for a clean stop; 1 when the
/// device ended with an error, which the recovery actions restart
/// (`sc.exe failureflag`).
pub fn exit_code(result: &Result<(), String>) -> u32 {
    match result {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// How often the service looks for its address at start.
pub const ADDRESS_POLL: Duration = Duration::from_millis(500);

/// Whether this box holds `ip` now: a socket binds to it only then.
pub fn address_held(ip: IpAddr) -> bool {
    UdpSocket::bind((ip, 0)).is_ok()
}

/// Before the device starts: waits, `ADDRESS_POLL` at a time, until
/// `held()` (this box holds the device's address). A service starts at
/// boot before the NIC has its address, and after a link reset the
/// address comes back later (dasio#1: the device ended with "no interface
/// of this box holds the address" and the SCM's retries gave up). Returns
/// false when a stop was asked first.
pub fn wait_for_address(
    mut held: impl FnMut() -> bool,
    stop: &AtomicBool,
    _pause: impl FnMut(Duration),
) -> bool {
    let _ = stop.load(Ordering::Acquire);
    held()
}

#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_carries_the_instance() {
        assert_eq!(name(1), "dante-asio-1");
        assert_eq!(name(16), "dante-asio-16");
    }

    #[test]
    fn stop_and_shutdown_stop_the_device_interrogate_reports() {
        assert_eq!(answer(Control::Stop), Answer::Stop);
        assert_eq!(answer(Control::Shutdown), Answer::Stop);
        assert_eq!(answer(Control::Interrogate), Answer::Report);
        assert_eq!(answer(Control::Other), Answer::NotImplemented);
    }

    #[test]
    fn an_error_exit_is_one_a_clean_stop_zero() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err("the device ended".into())), 1);
    }

    #[test]
    fn the_device_waits_for_its_address_until_the_box_holds_it() {
        let stop = AtomicBool::new(false);
        let mut looks = 0;
        let mut paused = Duration::ZERO;
        let held = wait_for_address(
            || {
                looks += 1;
                looks == 4
            },
            &stop,
            |d| paused += d,
        );
        assert!(held);
        assert_eq!(looks, 4);
        assert_eq!(paused, 3 * ADDRESS_POLL);
    }

    #[test]
    fn a_held_address_starts_the_device_at_once() {
        let stop = AtomicBool::new(false);
        let mut paused = Duration::ZERO;
        assert!(wait_for_address(|| true, &stop, |d| paused += d));
        assert_eq!(paused, Duration::ZERO);
    }

    #[test]
    fn a_stop_ends_the_wait_without_the_device() {
        let stop = AtomicBool::new(false);
        let mut looks = 0;
        let held = wait_for_address(
            || {
                looks += 1;
                if looks == 3 {
                    stop.store(true, Ordering::Release);
                }
                false
            },
            &stop,
            |_| {},
        );
        assert!(!held);
        assert_eq!(looks, 3);
    }

    #[test]
    fn the_box_holds_loopback_and_not_a_documentation_address() {
        assert!(address_held(IpAddr::from([127, 0, 0, 1])));
        assert!(!address_held(IpAddr::from([192, 0, 2, 1])));
    }

    #[test]
    fn the_stop_hint_covers_the_device_s_stop() {
        assert!(STOP_HINT > Duration::from_secs(7));
    }
}
