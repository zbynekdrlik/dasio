//! The device loop's look at its pump for the lost blocks by cause
//! (dasio#114): the Windows glue over the portable `crate::pump_loss`.

/// One look of `tracker` at the pump: its counts first, then its late
/// wakes, read whole or not at all (dasio#114). The counts come first:
/// the pump records a wake before it counts that wake's re-bootstrap.
pub(super) fn look_at_pump(
    tracker: &mut crate::pump_loss::Tracker,
    stats: &dstack::stream::PumpStats,
) -> crate::pump_loss::Lost {
    use crate::pump_loss::{Counts, Wake, Wakes};
    let v = stats.view();
    let counts = Counts {
        missed: v.missed,
        overruns: v.overruns,
        rebootstraps: v.rebootstraps,
        rebootstraps_late: v.rebootstraps_late,
    };
    let wakes = stats.wakes();
    let end = wakes.late_wakes();
    let recent: Vec<Wake> = wakes
        .recent()
        .iter()
        .map(|w| Wake {
            behind_us: w.behind_us,
            rebootstrap: w.rebootstrap,
        })
        .collect();
    let end_after = wakes.late_wakes();
    let kept = dstack::stream::wake::RECENT as u64;
    tracker.take(counts, Wakes::whole(end, &recent, end_after, kept))
}
