//! The DAW's sample rate (dasio#107): what `setSampleRate` does with the
//! rate a DAW asks. The device stays at the network's 96 kHz and converts
//! for an app at another of `shm::APP_RATES`; the driver only passes the
//! rate on (the instance's directory) and waits for the device to serve a
//! section at it. Portable, so it is tested on every OS.

use std::time::Duration;

/// The rate a DAW gets before it asks one, with no device (the network's).
pub const DEFAULT_RATE: u32 = 96_000;

/// The longest `setSampleRate` waits for the device to serve a section
/// at the asked rate (it stops its pump and starts it again on a new
/// section: tens of ms).
pub const SWITCH_WAIT: Duration = Duration::from_secs(5);

/// How often it looks meanwhile.
pub const SWITCH_POLL: Duration = Duration::from_millis(10);

/// What `setSampleRate` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to change: `ASE_OK` (the rate served, or 0: the SDK's
    /// "external clock", which keeps it).
    Keep,
    /// A rate no app may open the instance at: `ASE_NO_CLOCK`.
    Refuse,
    /// No device: the free-run takes the rate itself.
    Local(u32),
    /// Ask the device for the rate and wait for its section.
    Ask(u32),
}

/// What `setSampleRate(asked)` does while the instance serves `current`
/// Hz, with or without its `device`.
pub fn step(asked: f64, current: u32, device: bool) -> Step {
    if asked == 0.0 {
        return Step::Keep;
    }
    match shm::app_rate(asked) {
        None => Step::Refuse,
        Some(r) if r == current => Step::Keep,
        Some(r) if device => Step::Ask(r),
        Some(r) => Step::Local(r),
    }
}

/// Whether a section the device publishes serves the asked rate: another
/// session than the one held (the device served a new section) whose
/// header says `rate`.
pub fn served(asked: u32, held_session: u64, published_session: u64, rate: u32) -> bool {
    published_session != held_session && published_session != 0 && rate == asked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_already_served_or_zero_is_kept() {
        assert_eq!(step(96_000.0, 96_000, true), Step::Keep);
        assert_eq!(step(48_000.0, 48_000, false), Step::Keep);
        assert_eq!(step(0.0, 44_100, true), Step::Keep);
    }

    #[test]
    fn a_rate_no_app_may_open_is_refused() {
        for r in [22_050.0, 88_200.0, 192_000.0, 47_999.0, -48_000.0] {
            assert_eq!(step(r, 96_000, true), Step::Refuse, "{r}");
            assert_eq!(step(r, 96_000, false), Step::Refuse, "{r}");
        }
    }

    #[test]
    fn another_supported_rate_goes_to_the_device_or_the_free_run() {
        assert_eq!(step(48_000.0, 96_000, true), Step::Ask(48_000));
        assert_eq!(step(44_100.0, 48_000, true), Step::Ask(44_100));
        assert_eq!(step(96_000.0, 48_000, true), Step::Ask(96_000));
        assert_eq!(step(48_000.0, 96_000, false), Step::Local(48_000));
        assert_eq!(step(96_000.0, 44_100, false), Step::Local(96_000));
    }

    #[test]
    fn only_a_new_section_at_the_rate_serves_it() {
        assert!(served(48_000, 1, 2, 48_000));
        // The same session: the device has not served again yet.
        assert!(!served(48_000, 1, 1, 48_000));
        // A new section at another rate (a device restart meanwhile).
        assert!(!served(48_000, 1, 2, 96_000));
        // Nothing published.
        assert!(!served(48_000, 1, 0, 48_000));
    }

    #[test]
    fn the_wait_is_bounded_and_polled() {
        assert_eq!(DEFAULT_RATE, 96_000);
        assert!(SWITCH_POLL < SWITCH_WAIT);
        assert!(SWITCH_WAIT <= Duration::from_secs(5));
    }
}
