//! The probe's injected host stall (dasio#162): whether it ran as meant,
//! the host stalling while the device's pump kept publishing blocks, and
//! whether the driver counted the blocks the host missed meanwhile.
//!
//! The pump's lost time is a whole-run figure; the blocks it published
//! during the stall place it on the stall itself. A stall the pump did not
//! publish through cost the host nothing to miss: the runner took the
//! pump's CPU over it (CI 43983426049: 15 blocks lost to a 4.8 ms late
//! wake in that second, none to the device), which only the device's own
//! late-wake figures may excuse.

/// What the probe measured around its injected stall, at the host's rate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stall {
    /// The device blocks published while the host stalled (its seq after
    /// the stall less its seq before).
    pub published: u64,
    /// The stall's measured length, µs.
    pub stall_us: u64,
    /// The host's block, samples, and its rate, Hz.
    pub block: u32,
    pub rate: u32,
    /// The blocks the driver counted the host missing in the run.
    pub missed: u64,
    /// The pump's lost blocks in the run, by its own cause (dasio#114).
    pub pump_lost_late_blocks: u64,
    pub pump_lost_device_blocks: u64,
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
    if s.missed > 0 {
        Ok(Verdict::Exercised)
    } else {
        Err(format!(
            "the {} us host stall missed no block ({} published meanwhile)",
            s.stall_us, s.published
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
            stall_us: 5_005,
            block: 32,
            rate: 96_000,
            missed,
            pump_lost_late_blocks: late,
            pump_lost_device_blocks: device,
        }
    }

    #[test]
    fn a_5_ms_stall_spans_15_blocks_of_32_at_96_khz() {
        assert_eq!(stall_blocks(&stall(0, 0, 0, 0)), 15);
        assert_eq!(stall_blocks(&Stall::default()), 0);
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
        // CI 43983426049: nothing published in the 5 ms, 39 blocks lost to
        // late wakes in the run (15 in the stall's second), none to the
        // device.
        assert_eq!(verdict(&stall(0, 0, 39, 0)), Ok(Verdict::PumpCpuTaken));
        // One block published at the stall's edge, 13 lost late: covered.
        assert_eq!(verdict(&stall(1, 0, 13, 0)), Ok(Verdict::PumpCpuTaken));
    }

    #[test]
    fn a_stall_the_pump_did_not_run_through_without_the_runners_figures_fails() {
        // The device's own loss: never the runner's.
        assert!(verdict(&stall(0, 0, 39, 1)).is_err());
        // Too few late blocks to cover the stall.
        assert!(verdict(&stall(0, 0, 12, 0)).is_err());
        assert!(verdict(&stall(0, 0, 0, 0)).is_err());
    }
}
