//! The Windows calls of [`super`] (dasio#120): thin glue over the pure
//! parser and decision, compiled out on Linux (excluded from the mutation
//! gate, as the other `windows.rs` glue).

use tracing::{info, warn};

use super::{CpuSet, Decision, decide, parse, set_assigned};
use crate::pc_profile::{DEFAULT_PATH, Layout, site_instances, windows::cpu_count};

/// The system's CPU sets (`GetSystemCpuSetInformation`, this process
/// as the target).
pub fn read() -> Result<Vec<CpuSet>, String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemCpuSetInformation;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut len = 0u32;
    // SAFETY: a size query (no buffer); fails with
    // ERROR_INSUFFICIENT_BUFFER and sets `len`.
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, GetCurrentProcess(), 0)
    };
    if len == 0 {
        return Err(format!(
            "Windows gave no CPU set information: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: a buffer of `len` bytes; Windows writes at most that.
    if unsafe {
        GetSystemCpuSetInformation(
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
            GetCurrentProcess(),
            0,
        )
    } == 0
    {
        return Err(format!(
            "the CPU sets could not be read: {}",
            std::io::Error::last_os_error()
        ));
    }
    buf.truncate(len as usize);
    parse(&buf)
}

/// The process's default CPU sets as Windows holds them now.
fn read_back() -> Result<Vec<u32>, String> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessDefaultCpuSets};
    let mut ids = vec![0u32; 64];
    let mut n = 0u32;
    // SAFETY: this process's pseudo handle and a buffer of 64 ids.
    if unsafe { GetProcessDefaultCpuSets(GetCurrentProcess(), ids.as_mut_ptr(), 64, &mut n) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    ids.truncate(n as usize);
    Ok(ids)
}

/// Instance `instance`'s process takes its reserved CPUs as its
/// default CPU sets (dasio#120); logs what it set and what Windows
/// holds. Never fails the start: CPUs not reserved (no
/// `ReservedCpuSets`, or no reboot since `pc-profile apply`) are a
/// WARN and the process keeps its affinity alone, as before.
pub fn assign_at_start(instance: u32, bridge: bool) {
    let layout = match (
        cpu_count(),
        site_instances(std::path::Path::new(DEFAULT_PATH)),
    ) {
        (Ok(cpus), Ok(n)) => Layout::of(cpus, n),
        (Err(e), _) | (_, Err(e)) => {
            warn!(err = %e, "no CPU layout: the process's CPU sets are left as they are");
            return;
        }
    };
    let sets = match read() {
        Ok(s) => s,
        Err(e) => {
            warn!(err = %e, "the process's CPU sets are left as they are");
            return;
        }
    };
    match decide(&sets, &layout, instance as usize, bridge) {
        Decision::Assign { ids, cpus } => {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, SetProcessDefaultCpuSets,
            };
            // SAFETY: this process's pseudo handle and `ids.len()` ids.
            let ok = unsafe {
                SetProcessDefaultCpuSets(GetCurrentProcess(), ids.as_ptr(), ids.len() as u32)
            } != 0;
            if !ok {
                warn!(
                    err = %std::io::Error::last_os_error(),
                    cpus = format!("{cpus:#x}"),
                    "the process's default CPU sets were not set"
                );
                return;
            }
            set_assigned(cpus);
            match read_back() {
                Ok(held) => info!(
                    cpus = format!("{cpus:#x}"),
                    ids = ?ids,
                    held = ?held,
                    bridge,
                    "the process runs on its reserved CPUs (default CPU sets, dasio#120)"
                ),
                Err(e) => warn!(
                    cpus = format!("{cpus:#x}"),
                    ids = ?ids,
                    err = %e,
                    "default CPU sets set; reading them back failed"
                ),
            }
        }
        Decision::NotReserved { own, missing } => warn!(
            own = format!("{own:#x}"),
            missing = format!("{missing:#x}"),
            layout = %layout.describe(),
            "dasio's CPUs are not reserved: other processes' threads (Defender) can run on them; `dasio pc-profile apply` sets ReservedCpuSets, effective after a reboot (dasio#120). The process keeps its affinity alone"
        ),
        Decision::NoLayout(e) => warn!(
            err = %e,
            layout = %layout.describe(),
            "the layout has no CPUs for this instance: its CPU sets are left as they are"
        ),
    }
}
