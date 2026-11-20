#![cfg(windows)]
//! The process-wide count of locked bytes (dasio#28 E11), in a test binary
//! of its own: `cargo test` runs one process per binary, and this binary's
//! one test is the only one locking in it, so the count is exact here (in
//! `windows.rs`, other tests lock in parallel threads).

use shm::{Shape, Shared, locked_bytes};

// A dropped section gives its locked bytes back: the next lock raises the
// working set for what is still locked, not for sections long gone.
// Instance 88 belongs to this test.
#[test]
fn a_dropped_section_gives_its_locked_bytes_back() {
    assert_eq!(locked_bytes(), 0, "nothing locked yet");
    let mut first = Shared::create(88, Shape::new(4, 4, 256).unwrap(), 96_000, (88, 40)).unwrap();
    let a = first.lock().unwrap().bytes;
    assert!(a > 0);
    assert_eq!(locked_bytes(), a);
    let mut next = Shared::create(88, Shape::app(4, 4, 128).unwrap(), 48_000, (40, 60)).unwrap();
    let b = next.lock().unwrap().bytes;
    assert_eq!(locked_bytes(), a + b, "both sections, while both live");
    // Locking a locked section again adds nothing.
    next.lock().unwrap();
    assert_eq!(locked_bytes(), a + b);
    drop(first);
    assert_eq!(locked_bytes(), b, "the old section gave its bytes back");
    drop(next);
    assert_eq!(locked_bytes(), 0);
    // An unlocked section gives nothing back.
    let unlocked = Shared::create(88, Shape::new(1, 1, 32).unwrap(), 96_000, (56, 40)).unwrap();
    drop(unlocked);
    assert_eq!(locked_bytes(), 0);
}
