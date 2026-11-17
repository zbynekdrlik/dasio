//! The probe's injected host stall (dasio#162): whether it ran as meant,
//! the host stalling while the device's pump kept publishing blocks, and
//! whether the driver counted the blocks the host missed meanwhile.
//!
//! Every figure is the stall's own: the blocks the pump published while
//! the host stalled, the blocks the driver counted missed for it (at the
//! first switch after it), and the pump's lost time from the stall's start
//! to a few device ticks after it, not the whole run's. A stall the pump
//! did not publish through cost the host nothing to miss: the runner took
//! the pump's CPU over it (CI 43983426049: 15 blocks lost to a 4.8 ms late
//! wake in that second, none to the device), which only the device's own
//! late-wake figures may excuse.

/// What the probe measured around its injected stall, at the host's rate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stall {
    /// The device blocks published while the host stalled (its seq after
    /// the stall less its seq before).
    pub published: u64,
    /// The stall's asked length, µs (0: no stall asked).
    pub asked_us: u64,
    /// The stall's measured length, µs (0: it never ran).
    pub stall_us: u64,
    /// The host's block, samples.
    pub block: u32,
    /// The host's rate, Hz.
    pub rate: u32,
    /// The blocks the driver counted the host missing for the stall.
    pub missed: u64,
    /// The pump's lost blocks around the stall to its device's own late
    /// wakes (the runner's, dasio#114).
    pub lost_late_blocks: u64,
    /// The pump's lost blocks around the stall to anything else (the
    /// device's).
    pub lost_device_blocks: u64,
}

/// How the stall went when the run passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The pump published through the stall and the driver counted what
    /// the host missed.
    Exercised,
    /// The pump did not run through the stall and the device measured it
    /// as late wakes (the runner's), so there was nothing to miss.
    PumpCpuTaken,
}

/// The whole blocks the stall spans at the host's block.
pub fn stall_blocks(s: &Stall) -> u64 {
    if s.block == 0 {
        return 0;
    }
    s.stall_us * u64::from(s.rate) / 1_000_000 / u64::from(s.block)
}

/// The verdict on `s`, or why it fails.
pub fn verdict(s: &Stall) -> Result<Verdict, String> {
    // A stall that never ran, or ran short, tested nothing.
    if s.block == 0 || s.asked_us == 0 || s.stall_us < s.asked_us {
        return Err(format!(
            "the host stall did not run as asked: {} us of {} us at block {}",
            s.stall_us, s.asked_us, s.block
        ));
    }
    // The pump published through the stall: the host, back, takes the
    // newest block, and the driver counts every one before it missed.
    if s.published >= 2 {
        let due = s.published - 1;
        return if s.missed >= due {
            Ok(Verdict::Exercised)
        } else {
            Err(format!(
                "the {} us host stall missed no block: {} published meanwhile, {} counted missed (at least {due})",
                s.stall_us, s.published, s.missed
            ))
        };
    }
    // At most one block in the whole stall: the pump did not run. Only the
    // device's own late wakes, with none of its own losses, excuse it, and
    // they must cover the stall less what it published and one block of
    // phase in all (the stall need not start on a block's edge).
    let span = stall_blocks(s);
    if s.lost_device_blocks == 0 && s.lost_late_blocks + s.published + 1 >= span {
        Ok(Verdict::PumpCpuTaken)
    } else {
        Err(format!(
            "the {} us host stall missed no block: the pump published {} in it ({span} blocks), lost {} to late wakes and {} to the device around it",
            s.stall_us, s.published, s.lost_late_blocks, s.lost_device_blocks
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5 ms stall at block 32, 96 kHz (15 whole blocks).
    fn stall(published: u64, missed: u64, late: u64, device: u64) -> Stall {
        Stall {
            published,
            asked_us: 5_000,
            stall_us: 5_005,
            block: 32,
            rate: 96_000,
            missed,
            lost_late_blocks: late,
            lost_device_blocks: device,
        }
    }

    #[test]
    fn a_5_ms_stall_spans_15_blocks_of_32_at_96_khz() {
        assert_eq!(stall_blocks(&stall(0, 0, 0, 0)), 15);
        assert_eq!(stall_blocks(&Stall::default()), 0);
    }

    #[test]
    fn a_stall_that_never_ran_or_ran_short_fails_whatever_the_figures() {
        // Never met its moment: nothing measured.
        let never = Stall {
            asked_us: 5_000,
            block: 32,
            rate: 96_000,
            lost_late_blocks: 100,
            ..Stall::default()
        };
        assert!(verdict(&never).is_err());
        let short = Stall {
            stall_us: 4_999,
            ..stall(15, 14, 0, 0)
        };
        assert!(verdict(&short).is_err());
        let unasked = Stall {
            asked_us: 0,
            ..stall(15, 14, 0, 0)
        };
        assert!(verdict(&unasked).is_err());
        let no_block = Stall {
            block: 0,
            ..stall(15, 14, 0, 0)
        };
        assert!(verdict(&no_block).is_err());
        // Exactly as asked runs.
        let exact = Stall {
            stall_us: 5_000,
            ..stall(15, 14, 0, 0)
        };
        assert_eq!(verdict(&exact), Ok(Verdict::Exercised));
    }

    #[test]
    fn a_stall_the_pump_published_through_is_counted_missed() {
        assert_eq!(verdict(&stall(15, 14, 0, 0)), Ok(Verdict::Exercised));
        assert_eq!(verdict(&stall(15, 20, 0, 0)), Ok(Verdict::Exercised));
    }

    #[test]
    fn a_stall_the_pump_published_through_and_the_driver_missed_short_fails() {
        // 15 published and only 3 counted: the driver lost track of the
        // host's timeline, whatever the pump lost elsewhere.
        assert!(verdict(&stall(15, 3, 100, 0)).is_err());
        assert!(verdict(&stall(2, 0, 100, 0)).is_err());
    }

    #[test]
    fn a_stall_the_runner_took_the_pumps_cpu_over_is_its_own() {
        // CI 43983426049: nothing published in the 5 ms, 15 blocks lost to
        // the late wake in the stall's second, none to the device.
        assert_eq!(verdict(&stall(0, 0, 15, 0)), Ok(Verdict::PumpCpuTaken));
        // The boundary: 14 late and one block of phase cover 15.
        assert_eq!(verdict(&stall(0, 0, 14, 0)), Ok(Verdict::PumpCpuTaken));
        // One block published at the stall's edge, 13 lost late: covered.
        assert_eq!(verdict(&stall(1, 0, 13, 0)), Ok(Verdict::PumpCpuTaken));
    }

    #[test]
    fn a_stall_the_pump_did_not_run_through_without_the_runners_figures_fails() {
        // The device's own loss: never the runner's.
        assert!(verdict(&stall(0, 0, 39, 1)).is_err());
        // Too few late blocks to cover the stall.
        assert!(verdict(&stall(0, 0, 13, 0)).is_err());
        assert!(verdict(&stall(1, 0, 12, 0)).is_err());
        assert!(verdict(&stall(0, 0, 0, 0)).is_err());
    }
}
