//! The arithmetic of locking the device's views in memory (dasio#28 E11):
//! which bytes a lock covers and what working set the process needs for
//! them. Windows' `VirtualLock` locks whole pages and only up to the
//! process's minimum working set (less a few pages of its own), so the
//! device raises that first. The calls themselves are in `windows.rs`.

/// What the working set gets on top of the locked pages: the system's own
/// pages under the lock quota (8 on current Windows) and the pages the
/// device touches besides the views, so locking never pushes those out.
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

/// The working set (minimum, maximum) for `locked` bytes in locked pages
/// on top of the process's own `base` (its working set before any lock):
/// both raised by the locked bytes and [`LOCK_MARGIN`], the maximum never
/// below the minimum. Nothing locked: `base` as it is.
pub fn working_set(base: (usize, usize), locked: usize) -> (usize, usize) {
    if locked == 0 {
        return base;
    }
    let extra = locked.saturating_add(LOCK_MARGIN);
    let min = base.0.saturating_add(extra);
    (min, base.1.saturating_add(extra).max(min))
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
    fn the_working_set_grows_by_the_locked_bytes_and_the_margin() {
        let base = (200 << 10, 1380 << 10);
        let locked = 528 << 10;
        assert_eq!(
            working_set(base, locked),
            (base.0 + locked + LOCK_MARGIN, base.1 + locked + LOCK_MARGIN)
        );
    }

    #[test]
    fn nothing_locked_keeps_the_working_set() {
        assert_eq!(
            working_set((200 << 10, 1380 << 10), 0),
            (200 << 10, 1380 << 10)
        );
    }

    #[test]
    fn the_maximum_is_never_below_the_minimum() {
        // A base whose maximum lies below its minimum (never from Windows,
        // but the call must not ask for it).
        let (min, max) = working_set((3 << 20, 1 << 20), 4096);
        assert_eq!(min, (3 << 20) + 4096 + LOCK_MARGIN);
        assert_eq!(max, min);
    }

    #[test]
    fn a_huge_request_saturates_instead_of_wrapping() {
        assert_eq!(working_set((1, 2), usize::MAX), (usize::MAX, usize::MAX));
        assert_eq!(
            working_set((usize::MAX - 1, usize::MAX), 4096),
            (usize::MAX, usize::MAX)
        );
    }
}
