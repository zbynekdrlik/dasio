//! How often the driver may ask the DAW for a reset (`kAsioResetRequest`):
//! a device that keeps coming and going must not make the DAW reopen every
//! second. iemmixer's numbers: at most one request per [`MIN_GAP`] and
//! [`MAX`] per process. Portable, so it is tested on every OS.

use std::time::{Duration, Instant};

/// The shortest time between two requests.
pub const MIN_GAP: Duration = Duration::from_secs(5 * 60);
/// The most requests in a process's life.
pub const MAX: u32 = 3;

/// One instance's requests so far.
#[derive(Debug)]
pub struct Budget {
    count: u32,
    last: Option<Instant>,
}

impl Budget {
    pub const fn new() -> Budget {
        Budget {
            count: 0,
            last: None,
        }
    }

    /// Whether a request may go at `now`; one that may is counted.
    pub fn take(&mut self, now: Instant) -> bool {
        let gap_ok = self
            .last
            .is_none_or(|last| now.duration_since(last) >= MIN_GAP);
        if self.count < MAX && gap_ok {
            self.count += 1;
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

impl Default for Budget {
    fn default() -> Budget {
        Budget::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_are_iemmixer_s() {
        assert_eq!(MIN_GAP, Duration::from_secs(300));
        assert_eq!(MAX, 3);
    }

    #[test]
    fn one_request_per_gap() {
        let t = Instant::now();
        let mut b = Budget::new();
        assert!(b.take(t));
        assert!(!b.take(t + MIN_GAP - Duration::from_millis(1)));
        assert!(b.take(t + MIN_GAP));
    }

    #[test]
    fn at_most_max_requests() {
        let t = Instant::now();
        let mut b = Budget::new();
        let granted = (0..10).filter(|&k| b.take(t + MIN_GAP * k)).count() as u32;
        assert_eq!(granted, MAX);
    }

    #[test]
    fn a_refused_request_is_not_counted() {
        let t = Instant::now();
        let mut b = Budget::new();
        assert!(b.take(t));
        for k in 1..20 {
            assert!(!b.take(t + Duration::from_secs(k)));
        }
        assert!(b.take(t + MIN_GAP));
        assert!(b.take(t + 2 * MIN_GAP));
        assert!(!b.take(t + 3 * MIN_GAP));
    }
}
