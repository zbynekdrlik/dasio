//! The SCM glue: the dispatcher, the control handler and the status
//! reports around [`crate::run::run`].

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dstack::daemon::Place;
use dstack::host::ClockSource;
use tracing::{error, info, warn};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use super::{Answer, Control, STOP_HINT, address_held, answer, exit_code, name, wait_for_address};

/// What `dasio service` was started with.
struct Args {
    instance: u32,
    config: PathBuf,
    place: Place,
    clock: ClockSource,
    tuning: crate::Tuning,
}

static ARGS: OnceLock<Args> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Hands this process to the SCM until the service stops.
pub fn dispatch(
    instance: u32,
    config: PathBuf,
    place: Place,
    clock: ClockSource,
    tuning: crate::Tuning,
) -> Result<(), String> {
    ARGS.set(Args {
        instance,
        config,
        place,
        clock,
        tuning,
    })
    .map_err(|_| "the service was dispatched twice")?;
    info!(instance, service = %name(instance), "handing the process to the SCM");
    service_dispatcher::start(name(instance), ffi_service_main)
        .map_err(|e| format!("the service dispatcher: {e}"))
}

fn status(state: ServiceState, code: u32, wait: Duration) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: if code == 0 {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(code)
        },
        checkpoint: 0,
        wait_hint: wait,
        process_id: None,
    }
}

fn service_main(_args: Vec<OsString>) {
    let Some(args) = ARGS.get() else {
        error!("the service started without its arguments");
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&stop);
    let handle: Arc<OnceLock<service_control_handler::ServiceStatusHandle>> =
        Arc::new(OnceLock::new());
    let in_handler = Arc::clone(&handle);
    let handler = move |c: ServiceControl| -> ServiceControlHandlerResult {
        let control = match c {
            ServiceControl::Stop => Control::Stop,
            ServiceControl::Shutdown => Control::Shutdown,
            ServiceControl::Interrogate => Control::Interrogate,
            _ => Control::Other,
        };
        info!(?c, "a service control");
        match answer(control) {
            Answer::Stop => {
                asked.store(true, Ordering::Release);
                if let Some(h) = in_handler.get()
                    && let Err(e) =
                        h.set_service_status(status(ServiceState::StopPending, 0, STOP_HINT))
                {
                    warn!(%e, "StopPending not reported");
                }
                ServiceControlHandlerResult::NoError
            }
            Answer::Report => ServiceControlHandlerResult::NoError,
            Answer::NotImplemented => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let h = match service_control_handler::register(name(args.instance), handler) {
        Ok(h) => h,
        Err(e) => {
            error!(%e, "the control handler was not registered");
            return;
        }
    };
    let _ = handle.set(h);
    if let Err(e) = h.set_service_status(status(ServiceState::Running, 0, Duration::ZERO)) {
        error!(%e, "Running not reported");
    }
    info!(instance = args.instance, "service running");
    // At boot the service starts before the NIC has its address; after a
    // link reset it comes back later. The device starts once it is there.
    // An interface by name (`--iface`) is looked up each time: at boot it
    // may have no address yet. Its error is logged when it changes.
    let mut found = None;
    let mut said: Option<String> = None;
    let held = || match crate::place::address(&args.place) {
        Ok(ip) => {
            found = Some(ip);
            address_held(std::net::IpAddr::V4(ip))
        }
        Err(e) => {
            if said.as_deref() != Some(e.as_str()) {
                warn!(%e, "the device's interface has no address yet");
                said = Some(e);
            }
            false
        }
    };
    if !wait_for_address(held, &stop, std::thread::sleep) {
        info!("service stopped before the address came");
        if let Err(e) = h.set_service_status(status(ServiceState::Stopped, 0, Duration::ZERO)) {
            error!(%e, "Stopped not reported");
        }
        return;
    }
    let Some(ip) = found else {
        // wait_for_address returns true only after `held` said so, which
        // set `found`.
        error!("the device's address was held but not recorded");
        if let Err(e) = h.set_service_status(status(ServiceState::Stopped, 1, Duration::ZERO)) {
            error!(%e, "Stopped not reported");
        }
        return;
    };
    info!(%ip, place = ?args.place, "the device's address");
    let result = crate::run::run(
        args.instance,
        args.config.clone(),
        ip,
        args.clock,
        args.tuning,
        stop,
    );
    let code = exit_code(&result);
    match &result {
        Ok(()) => info!("service stopped"),
        Err(e) => error!(%e, code, "the device ended with an error"),
    }
    if let Err(e) = h.set_service_status(status(ServiceState::Stopped, code, Duration::ZERO)) {
        error!(%e, "Stopped not reported");
    }
}
