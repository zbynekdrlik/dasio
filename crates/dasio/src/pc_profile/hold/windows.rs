//! The SCM side of the hold (dasio#158): reads a held service's start type
//! and state, sets it back and stops it, in this process, through the
//! `windows-service` crate (the start type alone through
//! `ChangeServiceConfigW`: the crate's `change_config` wants every field).
//! A protected service whose start type the SCM refuses (DoSvc) gets it in
//! its registry key instead ([`super::registry_fallback`]).
//! The decisions and the log lines are the portable [`super`].

use std::time::{Duration, Instant};

use windows_service::service::{Service, ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_sys::Win32::System::Services::{ChangeServiceConfigW, SERVICE_NO_CHANGE};

use super::super::Value;
use super::super::windows::{read, write};
use super::{HELD, Held, decide, describe, not_held, set_start_with};

/// How long a pending state may last before the hold gives up on it.
const PENDING: Duration = Duration::from_secs(30);

/// Holds each of [`HELD`] at its start type and stopped (dasio#158); one log
/// line per service it had to fix, or per one it could not read or fix.
pub fn hold_services() -> Vec<String> {
    HELD.iter().filter_map(hold).collect()
}

/// `held` set back and stopped when needed; `None` when it is as it should
/// be or not installed.
fn hold(held: &Held) -> Option<String> {
    let key = format!(r"SYSTEM\CurrentControlSet\Services\{}", held.service);
    let start = match read(&key, "Start") {
        Ok(Some(Value::Dword(d))) => d,
        // Not installed: nothing to hold.
        Ok(None) => return None,
        Ok(Some(v)) => return Some(not_held(held, &format!("its Start is not a DWORD: {v:?}"))),
        Err(e) => return Some(not_held(held, &format!("its Start is unreadable: {e}"))),
    };
    // Read-only access first: a held service as it should be asks for no
    // more.
    let running = match open(held.service, ServiceAccess::QUERY_STATUS).and_then(|s| state(&s)) {
        Ok(s) => s != ServiceState::Stopped,
        Err(e) => return Some(not_held(held, &e)),
    };
    let fix = decide(held, Some(start), running)?;
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::CHANGE_CONFIG | ServiceAccess::STOP;
    let svc = match open(held.service, access) {
        Ok(s) => s,
        Err(e) => return Some(not_held(held, &e)),
    };
    let set = if fix.was.is_some() {
        set_start(&svc, &key, held.start)
    } else {
        Ok(())
    };
    let stopped = if fix.stop { stop(&svc) } else { Ok(()) };
    Some(describe(held, &fix, &set, &stopped))
}

/// A `windows-service` error with the OS error it carries.
fn os(e: windows_service::Error) -> String {
    match e {
        windows_service::Error::Winapi(io) => io.to_string(),
        e => e.to_string(),
    }
}

fn open(name: &str, access: ServiceAccess) -> Result<Service, String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("the service manager does not open: {}", os(e)))?;
    manager
        .open_service(name, access)
        .map_err(|e| format!("the service does not open: {}", os(e)))
}

fn state(svc: &Service) -> Result<ServiceState, String> {
    svc.query_status()
        .map(|s| s.current_state)
        .map_err(|e| format!("its state is unreadable: {}", os(e)))
}

/// The start type through the SCM, or, for a protected service the SCM
/// refuses ([`super::registry_fallback`]), in its registry key `key`.
fn set_start(svc: &Service, key: &str, start: u32) -> Result<(), String> {
    set_start_with(scm_set_start(svc, start), || {
        write(key, "Start", &Value::Dword(start))
    })
}

/// The start type through the SCM (as `sc config <name> start= ...`), so
/// the manager's own copy changes too, not only the registry's.
fn scm_set_start(svc: &Service, start: u32) -> Result<(), std::io::Error> {
    // SAFETY: the crate's open handle with SERVICE_CHANGE_CONFIG; every
    // other field unchanged (SERVICE_NO_CHANGE, nulls).
    let ok = unsafe {
        ChangeServiceConfigW(
            svc.raw_handle(),
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
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Waits (bounded) while `svc` is in `pending`; the state it ends in.
fn wait_out(svc: &Service, pending: ServiceState) -> Result<ServiceState, String> {
    let until = Instant::now() + PENDING;
    loop {
        let s = state(svc)?;
        if s != pending || Instant::now() >= until {
            return Ok(s);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Stops it and waits (bounded) for stopped. A service still starting
/// (right after boot) takes no stop request: wait for it to start first;
/// one already stopping is only waited for.
fn stop(svc: &Service) -> Result<(), String> {
    let s = wait_out(svc, ServiceState::StartPending)?;
    if s == ServiceState::StartPending {
        return Err("still starting after 30 s".into());
    }
    if s != ServiceState::Stopped
        && s != ServiceState::StopPending
        && let Err(e) = svc.stop()
    {
        // Stopped or stopping meanwhile is what we want.
        let now = state(svc)?;
        if now != ServiceState::Stopped && now != ServiceState::StopPending {
            return Err(format!("the stop request failed: {}", os(e)));
        }
    }
    match wait_out(svc, ServiceState::StopPending)? {
        ServiceState::Stopped => Ok(()),
        ServiceState::StopPending => Err("still stopping after 30 s".into()),
        s => Err(format!("{s:?} after the stop request")),
    }
}
