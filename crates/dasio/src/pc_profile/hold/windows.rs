//! The SCM side of the hold (dasio#158): reads a held service's start type
//! and state, sets it back and stops it, through the SCM API in this
//! process. The decisions are the portable [`super`].

use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW,
    QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP,
    SERVICE_NO_CHANGE, SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
};

use super::super::Value;
use super::super::windows::{read, wide};
use super::{HELD, decide, describe};

/// Holds each of [`HELD`] at its start type and stopped (dasio#158), in
/// this process (no child: the bridge kind runs it too); one log line per
/// service it had to fix, or per one it could not read or fix.
pub fn hold_services() -> Vec<String> {
    let mut lines = Vec::new();
    for held in HELD {
        let key = format!(r"SYSTEM\CurrentControlSet\Services\{}", held.service);
        let start = match read(&key, "Start") {
            Ok(Some(Value::Dword(d))) => d,
            // Not installed: nothing to hold.
            Ok(None) => continue,
            Ok(Some(v)) => {
                lines.push(format!(
                    "{}: its Start is not a DWORD ({v:?}), not held (dasio#158)",
                    held.service
                ));
                continue;
            }
            Err(e) => {
                lines.push(format!(
                    "{}: its Start is unreadable ({e}), not held (dasio#158)",
                    held.service
                ));
                continue;
            }
        };
        let svc = match Scm::open(held.service) {
            Ok(s) => s,
            Err(e) => {
                lines.push(format!("{}: not held (dasio#158): {e}", held.service));
                continue;
            }
        };
        let running = match svc.state() {
            Ok(s) => s != SERVICE_STOPPED,
            Err(e) => {
                lines.push(format!("{}: not held (dasio#158): {e}", held.service));
                continue;
            }
        };
        let Some(fix) = decide(held, Some(start), running) else {
            continue;
        };
        if fix.was.is_some()
            && let Err(e) = svc.set_start(held.start)
        {
            lines.push(format!(
                "{}: its start type was not set back (dasio#158): {e}",
                held.service
            ));
            continue;
        }
        let stopped = if fix.stop { svc.stop() } else { Ok(()) };
        lines.push(describe(held, &fix, &stopped));
    }
    lines
}

/// An open service and its manager, closed on drop.
struct Scm {
    manager: SC_HANDLE,
    service: SC_HANDLE,
}

impl Scm {
    fn open(name: &str) -> Result<Scm, String> {
        // SAFETY: the local manager, the active database (nulls).
        let manager =
            unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
        if manager.is_null() {
            return Err(format!(
                "the service manager does not open ({})",
                std::io::Error::last_os_error()
            ));
        }
        let n = wide(name);
        // SAFETY: an open manager and a NUL-terminated name.
        let service = unsafe {
            OpenServiceW(
                manager,
                n.as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG | SERVICE_STOP,
            )
        };
        if service.is_null() {
            let e = std::io::Error::last_os_error();
            // SAFETY: the manager opened above.
            unsafe { CloseServiceHandle(manager) };
            return Err(format!("the service does not open ({e})"));
        }
        Ok(Scm { manager, service })
    }

    /// Its current state (SERVICE_STOPPED, SERVICE_RUNNING, ...).
    fn state(&self) -> Result<u32, String> {
        // SAFETY: plain data, filled by the call.
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: an open service handle with SERVICE_QUERY_STATUS.
        if unsafe { QueryServiceStatus(self.service, &mut st) } == 0 {
            return Err(format!(
                "its state is unreadable ({})",
                std::io::Error::last_os_error()
            ));
        }
        Ok(st.dwCurrentState)
    }

    /// The start type through the SCM (as `sc config <name> start= ...`),
    /// so the manager's own copy changes too, not only the registry's.
    fn set_start(&self, start: u32) -> Result<(), String> {
        // SAFETY: an open service handle with SERVICE_CHANGE_CONFIG; every
        // other field unchanged (SERVICE_NO_CHANGE, nulls).
        let ok = unsafe {
            ChangeServiceConfigW(
                self.service,
                SERVICE_NO_CHANGE,
                start,
                SERVICE_NO_CHANGE,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    /// Asks it to stop and waits up to 30 s for stopped.
    fn stop(&self) -> Result<(), String> {
        // SAFETY: plain data, filled by the call.
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: an open service handle with SERVICE_STOP.
        if unsafe { ControlService(self.service, SERVICE_CONTROL_STOP, &mut st) } == 0 {
            let e = std::io::Error::last_os_error();
            // Stopped meanwhile (ERROR_SERVICE_NOT_ACTIVE) is what we want.
            if self.state()? == SERVICE_STOPPED {
                return Ok(());
            }
            return Err(format!("the stop request failed ({e})"));
        }
        for _ in 0..120 {
            if self.state()? == SERVICE_STOPPED {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("not stopped after 30 s".into())
    }
}

impl Drop for Scm {
    fn drop(&mut self) {
        // SAFETY: both handles opened in `open`, closed once here.
        unsafe {
            CloseServiceHandle(self.service);
            CloseServiceHandle(self.manager);
        }
    }
}
