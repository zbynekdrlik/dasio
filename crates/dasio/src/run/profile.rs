//! The PC profile's check (dasio#5): the DAW kind's in a child process
//! (moved out of run.rs); a bridge instance's in its own process without
//! the entries that start a process (dasio#120), since a child spawned
//! while an ASIO driver is loaded makes SoundGrid resync (dasio#106).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

/// The PC profile's checks (dasio#5): each problem is logged when it
/// appears and when it goes; the count is the tray's. The check runs in a
/// `dasio pc-profile check` child at normal priority on the CPUs outside
/// the media threads': on this process's own thread it held a CPU a media
/// thread's spin tail yielded, for a whole quantum (16 ms gaps at the
/// receivers every 10 min, FOH, dasio#1, 4.11.2026).
pub(crate) struct ProfileWatch {
    /// The media threads' CPUs, kept free of the child.
    own: usize,
    busy: Arc<AtomicBool>,
    last: Arc<Mutex<Option<Vec<String>>>>,
    /// Why the device must end: the last check found a must-have item
    /// missing (dasio#128).
    must: Arc<Mutex<Option<String>>>,
}

impl ProfileWatch {
    pub(crate) fn new(own: usize) -> Self {
        Self {
            own,
            busy: Arc::new(AtomicBool::new(false)),
            last: Arc::new(Mutex::new(None)),
            must: Arc::new(Mutex::new(None)),
        }
    }

    /// Why the device must end, when the last check found a must-have item
    /// of the baseline missing (dasio#128); the loop ends the device on it.
    pub(crate) fn must_have(&self) -> Option<String> {
        self.must.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Starts a check (none while one runs); the count lands in `problems`
    /// (u32::MAX without a profile).
    pub(super) fn start(&self, problems: &Arc<std::sync::atomic::AtomicU32>) {
        self.spawn(problems, child_check);
    }

    /// The bridge kind's check (dasio#120): in this process, on a thread at
    /// the lowest priority, without `[[bcd]]` and `[[task]]` (reading them
    /// starts bcdedit / schtasks, and a child process makes SoundGrid
    /// resync); `dasio pc-profile check` and the boot-time apply read those.
    pub(crate) fn start_in_process(&self, problems: &Arc<std::sync::atomic::AtomicU32>) {
        self.spawn(problems, in_process_check);
    }

    fn spawn(
        &self,
        problems: &Arc<std::sync::atomic::AtomicU32>,
        check: fn(usize) -> Result<crate::pc_profile::ChildCheck, String>,
    ) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (own, busy, last, must, problems) = (
            self.own,
            Arc::clone(&self.busy),
            Arc::clone(&self.last),
            Arc::clone(&self.must),
            Arc::clone(problems),
        );
        let spawned = std::thread::Builder::new()
            .name("pc-profile".into())
            .spawn(move || {
                // The built-in baseline applies on every PC, a site profile
                // or not (dasio#128).
                let found = check(own);
                let n = match found {
                    Ok(found) => {
                        if let crate::pc_profile::ChildCheck::Problems(p) = &found {
                            *must.lock().unwrap_or_else(|e| e.into_inner()) =
                                crate::pc_profile::must_have_failure(p);
                        }
                        record(&last, found)
                    }
                    Err(e) => {
                        warn!(%e, "the PC profile check did not run");
                        u32::MAX
                    }
                };
                problems.store(n, Ordering::Relaxed);
                busy.store(false, Ordering::Release);
            });
        if let Err(e) = spawned {
            warn!(%e, "the PC profile check's thread did not start");
            self.busy.store(false, Ordering::Release);
        }
    }
}

/// Logs what appeared and what went since the last check; the count.
fn record(last: &Mutex<Option<Vec<String>>>, found: crate::pc_profile::ChildCheck) -> u32 {
    use crate::pc_profile::ChildCheck;
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    let now = match found {
        ChildCheck::NoProfile(why) => {
            if last.is_none() {
                info!(path = crate::pc_profile::DEFAULT_PATH, %why, "no PC profile: its checks are off");
                *last = Some(Vec::new());
            }
            return u32::MAX;
        }
        ChildCheck::Problems(now) => now,
    };
    let before = last.take().unwrap_or_default();
    let (appeared, fixed) = crate::pc_profile::changes(&before, &now);
    for t in appeared {
        warn!("PC profile: {t}");
    }
    for t in fixed {
        info!("PC profile: fixed: {t}");
    }
    if now.is_empty() {
        info!("PC profile checked: as it should be");
    }
    let n = now.len() as u32;
    *last = Some(now);
    n
}

/// The check in this thread, at the lowest priority: the profile without
/// the entries whose reading starts a process
/// ([`crate::pc_profile::in_process`]).
fn in_process_check(_own: usize) -> Result<crate::pc_profile::ChildCheck, String> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST,
    };
    // SAFETY: this thread's pseudo handle.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST) } == 0 {
        warn!(err = %std::io::Error::last_os_error(), "the PC profile check runs at normal priority");
    }
    Ok(in_process_problems())
}

/// The profile's problems as read in this process, without the entries
/// whose reading starts a process.
fn in_process_problems() -> crate::pc_profile::ChildCheck {
    use crate::pc_profile::{
        ChildCheck, check, in_process, load,
        windows::{Pc, cpu_count},
    };
    let path = std::path::Path::new(crate::pc_profile::DEFAULT_PATH);
    let p = match load(path, cpu_count()) {
        Ok(p) => in_process(&p),
        Err(e) => return ChildCheck::NoProfile(e),
    };
    let pc = Pc::new(&p);
    ChildCheck::Problems(check(&p, &pc).iter().map(ToString::to_string).collect())
}

/// Before the device starts (dasio#128): a must-have item of the baseline
/// missing ends the start with its name. In this process: nothing is
/// loaded yet (no ASIO driver, no media thread). A profile that cannot be
/// read is left to the periodic check, which logs it.
pub(crate) fn must_have_at_start() -> Result<(), String> {
    match in_process_problems() {
        crate::pc_profile::ChildCheck::Problems(p) => {
            match crate::pc_profile::must_have_failure(&p) {
                Some(why) => {
                    tracing::error!(%why, "the device does not start");
                    Err(why)
                }
                None => Ok(()),
            }
        }
        crate::pc_profile::ChildCheck::NoProfile(_) => Ok(()),
    }
}

/// Runs `dasio pc-profile check` (this exe) at normal priority, without a
/// window, moved to the helper CPUs ([`crate::child_cpus`]) before it runs,
/// and reads its output.
fn child_check(own: usize) -> Result<crate::pc_profile::ChildCheck, String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessAffinityMask, SetProcessAffinityMask,
    };
    #[link(name = "ntdll")]
    unsafe extern "system" {
        /// Resumes every thread of a process (ntdll; no Win32 wrapper).
        fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
    }
    // The plain `dasio.exe` beside this instance's copy: Process Lasso's rule
    // for the instance's exe name would put the child back on the media
    // CPUs; `dasio.exe` falls under its catch-all.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plain = exe.with_file_name("dasio.exe");
    let exe = if plain.exists() { plain } else { exe };
    let child = std::process::Command::new(exe)
        .args(["pc-profile", "check"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(crate::child_creation_flags(own))
        .spawn()
        .map_err(|e| e.to_string())?;
    if own != 0 {
        // Started suspended: moved off the media CPUs first, then resumed.
        let (mut mine, mut system) = (0usize, 0usize);
        // SAFETY: this process's pseudo handle and two writable masks.
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut mine, &mut system) };
        let off = crate::child_cpus(mine, system, own);
        if off != 0 {
            info!(cpus = %format!("{off:#x}"), "the PC profile check runs on the helper CPUs");
            // SAFETY: the child's process handle, open while `child` lives.
            if unsafe { SetProcessAffinityMask(child.as_raw_handle(), off) } == 0 {
                warn!(err = %std::io::Error::last_os_error(), "the PC profile check stays on the media CPUs");
            }
        }
        // SAFETY: the child's process handle, suspended at its creation.
        let r = unsafe { NtResumeProcess(child.as_raw_handle()) };
        if r < 0 {
            let mut child = child;
            let _ = child.kill();
            return Err(format!(
                "the PC profile check did not resume (status {r:#x})"
            ));
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(crate::pc_profile::read_child_check(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    ))
}
