//! The PC profile's check for the DAW kind (dasio#5), in a child process
//! (moved out of run.rs; a bridge instance never starts it, dasio#106: a
//! child spawned while an ASIO driver is loaded makes SoundGrid resync).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

/// The PC profile's checks (dasio#5): each problem is logged when it
/// appears and when it goes; the count is the tray's. The check runs in a
/// `dasio pc-profile check` child at normal priority on the CPUs outside
/// the media threads': on this process's own thread it held a CPU a media
/// thread's spin tail yielded, for a whole quantum (16 ms gaps at the
/// receivers every 10 min, FOH, dasio#1, 4.11.2026).
pub(super) struct ProfileWatch {
    /// The media threads' CPUs, kept free of the child.
    own: usize,
    busy: Arc<AtomicBool>,
    last: Arc<Mutex<Option<Vec<String>>>>,
}

impl ProfileWatch {
    pub(super) fn new(own: usize) -> Self {
        Self {
            own,
            busy: Arc::new(AtomicBool::new(false)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts a check (none while one runs); the count lands in `problems`
    /// (u32::MAX without a profile).
    pub(super) fn start(&self, problems: &Arc<std::sync::atomic::AtomicU32>) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (own, busy, last, problems) = (
            self.own,
            Arc::clone(&self.busy),
            Arc::clone(&self.last),
            Arc::clone(problems),
        );
        let spawned = std::thread::Builder::new()
            .name("pc-profile".into())
            .spawn(move || {
                // No profile on this PC: no child to start.
                let path = std::path::Path::new(crate::pc_profile::DEFAULT_PATH);
                let found = if path.exists() {
                    child_check(own)
                } else {
                    Ok(crate::pc_profile::ChildCheck::NoProfile(
                        "the file does not exist".into(),
                    ))
                };
                let n = match found {
                    Ok(found) => record(&last, found),
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
    for t in now.iter().filter(|t| !before.contains(t)) {
        warn!("PC profile: {t}");
    }
    for t in before.iter().filter(|t| !now.contains(t)) {
        info!("PC profile: fixed: {t}");
    }
    if now.is_empty() {
        info!("PC profile checked: as it should be");
    }
    let n = now.len() as u32;
    *last = Some(now);
    n
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
