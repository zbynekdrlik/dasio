// SPDX-License-Identifier: GPL-3.0-or-later

//! The ASIO `systemTime` (dasio#28, E12). The ASIO SDK says it "must be
//! derived from timeGetTime()" on Windows: a host may compare it with its
//! own `timeGetTime()`. timeGetTime moves in steps of 1 ms or more, so the
//! stamp is the performance counter's time shifted into timeGetTime's base
//! and wrap: the same epoch, with the counter's resolution.

/// timeGetTime wraps after 2^32 ms; so does the stamp.
pub const WRAP_NS: u64 = (1u64 << 32) * 1_000_000;

/// The shift from counter ns into timeGetTime's base, from a tick edge:
/// timeGetTime read `ms` for the first time at counter time `qpc_ns`.
pub fn offset(ms: u32, qpc_ns: u64) -> u64 {
    let ms_ns = u64::from(ms) * 1_000_000;
    (ms_ns + WRAP_NS - qpc_ns % WRAP_NS) % WRAP_NS
}

/// The stamp at counter time `qpc_ns`, in timeGetTime's base and wrap.
pub fn stamp(qpc_ns: u64, offset: u64) -> u64 {
    (qpc_ns % WRAP_NS + offset) % WRAP_NS
}

/// The shift measured now: waits for the next timeGetTime tick (one tick at
/// most, 50 ms at the very most), off the audio path.
#[cfg(windows)]
pub fn calibrate() -> u64 {
    use windows_sys::Win32::Media::timeGetTime;
    // SAFETY: plain calls.
    let first = unsafe { timeGetTime() };
    let deadline = shm::qpc_ns() + 50_000_000;
    loop {
        // SAFETY: plain call.
        let ms = unsafe { timeGetTime() };
        let now = shm::qpc_ns();
        if ms != first || now > deadline {
            return offset(ms, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_at_the_edge_is_the_tick() {
        for (ms, qpc) in [
            (0, 0),
            (1_234, 987_654_321),
            (u32::MAX, 5),
            (7, WRAP_NS + 3),
        ] {
            assert_eq!(stamp(qpc, offset(ms, qpc)), u64::from(ms) * 1_000_000);
        }
    }

    #[test]
    fn the_stamp_runs_with_the_counter() {
        let o = offset(1_000, 50_000_000);
        assert_eq!(stamp(50_000_000 + 250_000, o), 1_000_250_000);
        assert_eq!(stamp(50_000_000 + 7_000_001, o), 1_007_000_001);
    }

    #[test]
    fn the_stamp_wraps_where_time_get_time_does() {
        let o = offset(u32::MAX, 1_000);
        // One ms after the last ms before the wrap: 0 again.
        assert_eq!(stamp(1_000 + 1_000_000, o), 0);
        assert_eq!(stamp(1_000 + 1_500_000, o), 500_000);
    }

    /// On the real clocks: the stamp is within a tick of timeGetTime and
    /// resolves below a millisecond.
    #[cfg(windows)]
    #[test]
    fn the_stamp_follows_time_get_time_finer() {
        use windows_sys::Win32::Media::timeGetTime;
        let o = calibrate();
        for _ in 0..200 {
            // SAFETY: plain call.
            let before = u64::from(unsafe { timeGetTime() }) * 1_000_000;
            let s = stamp(shm::qpc_ns(), o);
            // SAFETY: plain call.
            let after = u64::from(unsafe { timeGetTime() }) * 1_000_000;
            if after < before {
                continue; // the 49.7-day wrap between the reads
            }
            assert!(
                s + 20_000_000 >= before && s <= after + 20_000_000,
                "{before} <= {s} <= {after}, within 20 ms"
            );
        }
        let a = stamp(shm::qpc_ns(), o);
        let t = shm::qpc_ns();
        while shm::qpc_ns() < t + 100_000 {}
        let b = stamp(shm::qpc_ns(), o);
        assert!(b > a && b - a < 1_000_000, "100 µs apart: {a} then {b}");
    }
}
