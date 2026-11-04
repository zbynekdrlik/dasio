//! `dasio jitter --cpu <n> --seconds <s>`: how long Windows takes a CPU
//! away from a thread at priority 31 (a REALTIME process, TIME_CRITICAL),
//! pinned to `cpu`: the thread reads QPC in a loop and counts every gap
//! between two reads above 50 µs, 500 µs and 2 ms, and the longest. What it
//! finds is interrupts, DPCs and anything else above the device's threads.
//! Never on a CPU a DAW uses: the thread holds its CPU the whole run.

use std::time::{Duration, Instant};

/// What a run found on one CPU.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Jitter {
    pub cpu: usize,
    pub over_50us: u64,
    pub over_500us: u64,
    pub over_2ms: u64,
    pub longest_us: u64,
}

/// Runs the measuring thread on `cpu` for `seconds`.
pub fn jitter(cpu: usize, seconds: u64) -> Result<Jitter, String> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, REALTIME_PRIORITY_CLASS, SetPriorityClass,
        SetProcessAffinityMask, SetThreadAffinityMask, SetThreadPriority,
        THREAD_PRIORITY_TIME_CRITICAL,
    };
    if cpu >= 64 {
        return Err("CPU past the first group".into());
    }
    std::thread::spawn(move || {
        // SAFETY: this process's and this thread's pseudo handles.
        unsafe {
            SetPriorityClass(GetCurrentProcess(), REALTIME_PRIORITY_CLASS);
            // The process's set first: a thread cannot leave it (error 87).
            SetProcessAffinityMask(GetCurrentProcess(), 1usize << cpu);
            if SetThreadAffinityMask(GetCurrentThread(), 1usize << cpu) == 0 {
                return Err(format!("CPU {cpu}: {}", std::io::Error::last_os_error()));
            }
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
        let mut j = Jitter {
            cpu,
            ..Jitter::default()
        };
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut last = Instant::now();
        while last < end {
            let now = Instant::now();
            let gap = now.duration_since(last).as_micros() as u64;
            if gap > 50 {
                j.over_50us += 1;
                if gap > 500 {
                    j.over_500us += 1;
                }
                if gap > 2_000 {
                    j.over_2ms += 1;
                }
                j.longest_us = j.longest_us.max(gap);
            }
            last = now;
        }
        Ok(j)
    })
    .join()
    .map_err(|_| "the measuring thread panicked".to_string())?
}
