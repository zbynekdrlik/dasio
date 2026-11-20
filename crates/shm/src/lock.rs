//! The arithmetic of locking the device's views in memory (dasio#28 E11):
//! which bytes a lock covers and what working set the process needs for
//! them. Windows' `VirtualLock` locks whole pages and only up to the
//! process's minimum working set (less a few pages of its own), so the
//! device raises that first. The calls themselves are in `windows.rs`.

/// What the working set gets on top of the locked pages: the system's own
/// pages under the lock quota (8 on current Windows) and some headroom. The
/// pages the device uses besides the views are the maximum's `now` term in
/// [`working_set`], not this margin.
pub const LOCK_MARGIN: usize = 1 << 20;

/// The bytes of the whole pages (`page` bytes each) that hold `len` bytes
/// from address `addr`: what a lock of that range locks. 0 for an empty
/// range or a page size of 0.
pub fn page_span(addr: usize, len: usize, page: usize) -> usize {
    if len == 0 || page == 0 {
        return 0;
    }
    let first = addr / page;
    let last = addr.saturating_add(len - 1) / page;
    (last - first + 1).saturating_mul(page)
}

/// The working set (minimum, maximum) for `locked` bytes in locked pages.
/// `base`: the process's own limits before its first lock; `now`: its
/// working set in use and its peak, bytes. The minimum (`VirtualLock`'s
/// quota) is the base minimum plus the locked bytes and [`LOCK_MARGIN`]:
/// it is guaranteed resident memory, so it does not take the peak too,
/// which would make the raise likelier to fail. The maximum is the same
/// extra on top of the larger of the base maximum and the use: the device
/// process holds tens of MB (dstack's buffers, Npcap, stacks) against
/// Windows' default maximum of about 1.4 MB, and a maximum below its use
/// would trim its unlocked pages, faulting them back on the pump. The
/// maximum is never below the minimum. Nothing locked: `base`'s minimum,
/// the maximum still never below the use.
pub fn working_set(base: (usize, usize), now: (usize, usize), locked: usize) -> (usize, usize) {
    let used = now.0.max(now.1);
    if locked == 0 {
        return (base.0, base.1.max(used));
    }
    let extra = locked.saturating_add(LOCK_MARGIN);
    let min = base.0.saturating_add(extra);
    (min, base.1.max(used).saturating_add(extra).max(min))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 4096;

    #[test]
    fn a_range_locks_the_whole_pages_it_touches() {
        // From a page boundary: rounded up to whole pages.
        assert_eq!(page_span(0x1_0000, 1, PAGE), PAGE);
        assert_eq!(page_span(0x1_0000, PAGE, PAGE), PAGE);
        assert_eq!(page_span(0x1_0000, PAGE + 1, PAGE), 2 * PAGE);
        assert_eq!(page_span(0x1_0000, 10 * PAGE, PAGE), 10 * PAGE);
        // From inside a page: the first page counts too.
        assert_eq!(page_span(0x1_0000 + 100, PAGE, PAGE), 2 * PAGE);
        assert_eq!(page_span(0x1_0000 + PAGE - 1, 2, PAGE), 2 * PAGE);
        assert_eq!(page_span(0x1_0000 + 100, PAGE - 100, PAGE), PAGE);
        // A page boundary far from the start (a whole page index > 1).
        assert_eq!(page_span(7 * PAGE + 5, 3 * PAGE, PAGE), 4 * PAGE);
    }

    #[test]
    fn an_empty_range_or_no_page_size_locks_nothing() {
        assert_eq!(page_span(0x1_0000, 0, PAGE), 0);
        assert_eq!(page_span(0x1_0000, 10, 0), 0);
    }

    #[test]
    fn a_range_at_the_top_of_the_address_space_does_not_overflow() {
        assert_eq!(page_span(usize::MAX - 10, 100, PAGE), PAGE);
        assert_eq!(page_span(usize::MAX, usize::MAX, 1 << 63), 1 << 63);
    }

    #[test]
    fn the_device_s_section_locks_its_size_in_pages() {
        // The views start on the system's 64 KiB allocation boundary.
        let shape = crate::Shape::new(64, 64, 256).unwrap();
        let size = shape.size();
        assert_eq!(
            page_span(0x20_0000, size, PAGE),
            size.next_multiple_of(PAGE)
        );
    }

    #[test]
    fn the_margin_is_one_mib_on_top_of_use_and_lock() {
        assert_eq!(LOCK_MARGIN, 1_048_576);
        // In literal bytes, so the margin's value counts, not its name:
        // 40 MiB in use, 1 MiB locked, 1 MiB margin.
        assert_eq!(
            working_set((204_800, 1_413_120), (41_943_040, 0), 1_048_576),
            (2_301_952, 44_040_192)
        );
    }

    /// Windows' default limits.
    const BASE: (usize, usize) = (200 << 10, 1380 << 10);

    #[test]
    fn the_maximum_grows_by_the_locked_bytes_and_the_margin_over_the_use() {
        let locked = 528 << 10;
        let extra = locked + LOCK_MARGIN;
        // In use 40 MB, peak 52 MB: the peak counts for the maximum; the
        // minimum is the base's plus the lock.
        assert_eq!(
            working_set(BASE, (40 << 20, 52 << 20), locked),
            (BASE.0 + extra, (52 << 20) + extra)
        );
        // In use above the peak read (two reads): the use counts.
        assert_eq!(
            working_set(BASE, (60 << 20, 52 << 20), locked),
            (BASE.0 + extra, (60 << 20) + extra)
        );
    }

    #[test]
    fn a_maximum_above_the_use_is_the_one_raised() {
        let locked = 528 << 10;
        let extra = locked + LOCK_MARGIN;
        assert_eq!(
            working_set((8 << 20, 64 << 20), (6 << 20, 7 << 20), locked),
            ((8 << 20) + extra, (64 << 20) + extra)
        );
        assert_eq!(
            working_set((100 << 10, 9 << 20), (6 << 20, 7 << 20), locked),
            ((100 << 10) + extra, (9 << 20) + extra)
        );
    }

    #[test]
    fn nothing_locked_keeps_the_minimum_and_never_lowers_the_maximum_below_the_use() {
        assert_eq!(working_set(BASE, (100 << 10, 150 << 10), 0), BASE);
        assert_eq!(
            working_set(BASE, (30 << 20, 40 << 20), 0),
            (BASE.0, 40 << 20)
        );
        assert_eq!(
            working_set(BASE, (50 << 20, 40 << 20), 0),
            (BASE.0, 50 << 20)
        );
    }

    #[test]
    fn the_maximum_is_never_below_the_minimum() {
        // A base whose maximum lies below its minimum (never from Windows,
        // but the call must not ask for it).
        let (min, max) = working_set((3 << 20, 1 << 20), (4096, 8192), 4096);
        assert_eq!(min, (3 << 20) + 4096 + LOCK_MARGIN);
        assert_eq!(max, min);
    }

    #[test]
    fn a_huge_request_saturates_instead_of_wrapping() {
        assert_eq!(
            working_set((1, 2), (3, 4), usize::MAX),
            (usize::MAX, usize::MAX)
        );
        assert_eq!(
            working_set((usize::MAX - 1, usize::MAX), (0, 0), 4096),
            (usize::MAX, usize::MAX)
        );
    }
}
