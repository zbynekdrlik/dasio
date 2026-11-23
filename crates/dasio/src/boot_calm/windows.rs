//! The Windows reads of the boot wait ([`super`]): the system uptime
//! (`GetTickCount64`), the CPU times (`GetSystemTimes`) and the disks'
//! average queue (one PDH query on `\PhysicalDisk(_Total)\Avg. Disk Queue
//! Length`, English counter names so a localized Windows reads it too).
//! Documented calls only; a disk queue that cannot be read is `None`, and
//! the CPU alone decides then.

use std::time::Duration;

use tracing::warn;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::GetSystemTimes;

use super::{CpuTimes, Sample, busy_percent};

/// The counter the disk queue is read from (PDH's English name).
const DISK_QUEUE: &str = "\\PhysicalDisk(_Total)\\Avg. Disk Queue Length";

/// The time since the boot.
pub fn uptime() -> Duration {
    // SAFETY: no arguments, no failure.
    Duration::from_millis(unsafe { GetTickCount64() })
}

/// The PC's CPU times now; `None` if the call fails.
pub fn cpu_times() -> Option<CpuTimes> {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (z, z, z);
    // SAFETY: three valid FILETIMEs to write.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let t = |f: FILETIME| crate::cpu_snap::filetime(f.dwHighDateTime, f.dwLowDateTime);
    Some(CpuTimes {
        idle: t(idle),
        kernel: t(kernel),
        user: t(user),
    })
}

/// An open PDH query on [`DISK_QUEUE`], closed when dropped.
struct DiskQueue {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl DiskQueue {
    /// The query, its counter added and its first collection taken (an
    /// average needs two); the step and PDH status that failed.
    fn open() -> Result<DiskQueue, (&'static str, u32)> {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: no data source (the live counters), a handle to write.
        let st = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
        if st != 0 {
            return Err(("PdhOpenQueryW", st));
        }
        // From here the query closes on every path (Drop).
        let mut q = DiskQueue {
            query,
            counter: std::ptr::null_mut(),
        };
        let path: Vec<u16> = DISK_QUEUE.encode_utf16().chain(Some(0)).collect();
        // SAFETY: an open query, a NUL-terminated path, a handle to write.
        let st = unsafe { PdhAddEnglishCounterW(q.query, path.as_ptr(), 0, &mut q.counter) };
        if st != 0 {
            return Err(("PdhAddEnglishCounterW", st));
        }
        // SAFETY: an open query.
        let st = unsafe { PdhCollectQueryData(q.query) };
        if st != 0 {
            return Err(("PdhCollectQueryData", st));
        }
        Ok(q)
    }

    /// The average queue since the last read; `None` when PDH gives no
    /// valid value (e.g. the first one on a just-booted PC).
    fn read(&mut self) -> Option<f64> {
        // SAFETY: an open query.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: the query's counter, a value to write, no type wanted.
        let st = unsafe {
            PdhGetFormattedCounterValue(
                self.counter,
                PDH_FMT_DOUBLE,
                std::ptr::null_mut(),
                &mut value,
            )
        };
        if st != 0
            || (value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA)
        {
            return None;
        }
        // SAFETY: PDH_FMT_DOUBLE fills the double.
        Some(unsafe { value.Anonymous.doubleValue })
    }
}

impl Drop for DiskQueue {
    fn drop(&mut self) {
        // SAFETY: the query this value opened, closed once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// The wait's reader: one [`Sample`] a call, the CPU busy and the disk
/// queue since the last call (the first since [`Reader::open`]).
pub struct Reader {
    last: Option<CpuTimes>,
    disk: Option<DiskQueue>,
    /// A failed `GetSystemTimes` was warned (once a wait).
    cpu_warned: bool,
}

impl Reader {
    /// The CPU times now and the disk query; a query that cannot be opened
    /// is a WARN, and the CPU alone decides.
    pub fn open() -> Reader {
        let disk = match DiskQueue::open() {
            Ok(q) => Some(q),
            Err((step, status)) => {
                warn!(
                    step,
                    status = format!("{status:#x}"),
                    "the disk queue cannot be read: the CPU alone decides whether Windows is calm"
                );
                None
            }
        };
        Reader {
            last: cpu_times(),
            disk,
            cpu_warned: false,
        }
    }

    /// The PC since the last call. A CPU that cannot be read is never
    /// calm: the wait then runs to its cap, said once here.
    pub fn read(&mut self) -> Sample {
        let now = cpu_times();
        if now.is_none() && !self.cpu_warned {
            self.cpu_warned = true;
            warn!(
                err = %std::io::Error::last_os_error(),
                "the CPU times cannot be read (GetSystemTimes): Windows is never judged calm, the device starts at the cap"
            );
        }
        let cpu_percent = self
            .last
            .zip(now)
            .and_then(|(before, after)| busy_percent(before, after));
        self.last = now;
        Sample {
            cpu_percent,
            disk_queue: self.disk.as_mut().and_then(DiskQueue::read),
        }
    }
}
