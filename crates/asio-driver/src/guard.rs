//! What keeps the DAW alive whatever goes wrong inside the driver: a panic
//! never unwinds into the host (in an `extern "system"` method it would
//! abort the DAW's process), a poisoned lock is still usable, and a wait for
//! the worker thread is bounded. Portable, so it is tested on every OS.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Runs `f`; a panic inside it returns `fallback` instead.
pub fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// The lock, also after a panic poisoned it (every value it guards stays
/// valid on its own).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waits up to `bound` for the thread to end; the handle back when it did
/// not (the caller abandons it).
pub fn join_within<T>(h: JoinHandle<T>, bound: Duration) -> Result<(), JoinHandle<T>> {
    let until = Instant::now() + bound;
    while !h.is_finished() {
        if Instant::now() >= until {
            return Err(h);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = h.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_panic_returns_the_fallback() {
        assert_eq!(guard(-1, || -> i32 { panic!("inside the driver") }), -1);
        assert_eq!(guard(-1, || 7), 7);
    }

    #[test]
    fn a_poisoned_lock_is_still_usable() {
        let m = Arc::new(Mutex::new(5));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poisons the lock");
        })
        .join();
        assert!(m.is_poisoned());
        *lock(&m) += 1;
        assert_eq!(*lock(&m), 6);
    }

    #[test]
    fn a_thread_that_ends_is_joined() {
        let h = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(20)));
        assert!(join_within(h, Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn a_thread_that_does_not_end_is_handed_back_after_the_bound() {
        let go = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&go);
        // Ends on its own after 3 s at the latest, so a wait that ignores
        // the bound fails the test instead of hanging it.
        let h = std::thread::spawn(move || {
            let t = Instant::now();
            while !g.load(Ordering::Acquire) && t.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let t = Instant::now();
        let back = join_within(h, Duration::from_millis(100));
        let took = t.elapsed();
        assert!(back.is_err());
        assert!(took >= Duration::from_millis(100), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        go.store(true, Ordering::Release);
        back.unwrap_err().join().unwrap();
    }
}
