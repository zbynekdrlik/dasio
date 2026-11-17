//! The PC as the profile sees it: HKLM values (with the Dante NIC's keys
//! resolved), ini lines, running processes and free disk space; and the
//! writes `apply` makes (power-plan values through powercfg, dasio#128);
//! what an `[[absent]]` entry names and its removal (dasio#128).

use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ,
    REG_QWORD, REG_SZ, RRF_RT_ANY, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    REALTIME_PRIORITY_CLASS,
};

use super::absent::{self, AbsentEntry, Fallback, Removal, Step, Target};
use super::{Action, ProcState, Profile, Reader, Value, get_ini, set_ini};

const NET_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

/// The PC's logical CPUs, every processor group (the CPU layout's count,
/// dasio#128): never the calling process's affinity, which Process Lasso
/// narrows for dasio's own processes. An error when Windows gives none:
/// a layout for one CPU would put everything on CPU 0.
pub fn cpu_count() -> Result<usize, String> {
    // SAFETY: no pointers; ALL_PROCESSOR_GROUPS counts every group.
    let n = unsafe {
        windows_sys::Win32::System::Threading::GetActiveProcessorCount(
            windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
        )
    };
    if n == 0 {
        return Err(format!(
            "the PC's CPU count is unreadable: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(n as usize)
}

/// The PC, with the Dante NIC's keys found from the profile's GUID.
pub struct Pc {
    nic_class: Option<String>,
    nic_device: Option<String>,
    /// The Dante NIC's driver service (its Enum key's `Service`).
    nic_service: Option<String>,
    /// The Dante NIC's hardware IDs (its Enum key's `HardwareID`).
    nic_hardware_ids: Vec<String>,
}

impl Pc {
    pub fn new(p: &Profile) -> Pc {
        let nic_class = p.nic_guid.as_deref().and_then(find_nic_class);
        let instance = nic_class
            .as_deref()
            .and_then(|k| match read(k, "DeviceInstanceID") {
                Ok(Some(Value::String(id))) => Some(id),
                _ => None,
            });
        let nic_device = instance
            .as_deref()
            .map(|id| format!(r"SYSTEM\CurrentControlSet\Enum\{id}\Device Parameters"));
        let nic_service = instance.as_deref().and_then(|id| {
            match read(&format!(r"SYSTEM\CurrentControlSet\Enum\{id}"), "Service") {
                Ok(Some(Value::String(s))) => Some(s),
                _ => None,
            }
        });
        let nic_hardware_ids = instance
            .as_deref()
            .map(|id| {
                match read(
                    &format!(r"SYSTEM\CurrentControlSet\Enum\{id}"),
                    "HardwareID",
                ) {
                    Ok(Some(Value::MultiSz(ids))) => ids,
                    _ => Vec::new(),
                }
            })
            .unwrap_or_default();
        Pc {
            nic_class,
            nic_device,
            nic_service,
            nic_hardware_ids,
        }
    }

    /// `key` (as the profile writes it) as a path under HKLM.
    fn resolve(&self, key: &str) -> Result<String, String> {
        let k = if let Some(rest) = key.strip_prefix("{nic_class}") {
            format!(
                "{}{rest}",
                self.nic_class
                    .as_deref()
                    .ok_or("the Dante NIC's class key was not found")?
            )
        } else if let Some(rest) = key.strip_prefix(super::power::ACTIVE_SCHEME) {
            // The active power plan's own key (dasio#128): the baseline
            // sets its processor and power values, whichever plan it is.
            let scheme = match read(
                r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes",
                "ActivePowerScheme",
            ) {
                Ok(Some(Value::String(g))) => g,
                _ => return Err("the active power plan was not found".into()),
            };
            format!(r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\{scheme}{rest}")
        } else if let Some(rest) = key.strip_prefix("{nic_device}") {
            format!(
                "{}{rest}",
                self.nic_device
                    .as_deref()
                    .ok_or("the Dante NIC's device key was not found")?
            )
        } else {
            key.strip_prefix(r"HKLM\")
                .ok_or_else(|| format!("{key}: only HKLM\\ keys"))?
                .to_string()
        };
        Ok(k)
    }

    /// Applies one action; the error says why it failed.
    pub fn apply(&self, a: &Action) -> Result<(), String> {
        // A power-plan value goes through powercfg (dasio#128): a plain
        // write leaves a value the scheme never stored absent.
        if let Some(cmds) = super::powercfg_commands(a) {
            return cmds.iter().try_for_each(|args| powercfg(args.as_slice()));
        }
        match a {
            Action::Reg { key, name, value } => {
                let k = self.resolve(key)?;
                write(&k, name, value)
            }
            Action::Ini { file, key, value } => {
                let path = Path::new(file);
                let text = read_text(path)?;
                std::fs::write(path, set_ini(&text, key, value)).map_err(|e| format!("{file}: {e}"))
            }
            Action::Remove(r) => remove(r),
        }
    }
}

impl Reader for Pc {
    fn nic_service(&self) -> Option<String> {
        self.nic_service.clone()
    }

    fn nic_hardware_ids(&self) -> Vec<String> {
        self.nic_hardware_ids.clone()
    }

    fn absent(&self, e: &AbsentEntry) -> Result<Vec<String>, String> {
        let one = |there: bool, what: &str| {
            if there {
                vec![what.to_string()]
            } else {
                Vec::new()
            }
        };
        match e.target() {
            Some(Target::Exe(exe)) => Ok(one(file_there(exe)?, exe)),
            Some(Target::Service(s)) => Ok(one(
                key_exists(&format!(r"SYSTEM\CurrentControlSet\Services\{s}"))?,
                s,
            )),
            Some(Target::Task(t)) => tasks_matching(t),
            None => Err("the entry names nothing".into()),
        }
    }

    fn reg(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
        read(&self.resolve(key)?, name)
    }

    fn ini(&self, file: &str, key: &str) -> Result<Option<String>, String> {
        let text = read_text(Path::new(file))?;
        Ok(get_ini(&text, key))
    }

    fn process(&self, exe: &str) -> Option<ProcState> {
        let pid = find_process(exe)?;
        // SAFETY: a query-only handle, closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let (mut mask, mut system) = (0usize, 0usize);
        // SAFETY: a valid handle and two writable masks.
        let ok = unsafe { GetProcessAffinityMask(h, &mut mask, &mut system) } != 0;
        // SAFETY: a valid handle.
        let class = unsafe { GetPriorityClass(h) };
        // SAFETY: the handle opened above, closed once.
        unsafe { CloseHandle(h) };
        if !ok || class == 0 {
            return None;
        }
        let priority = match class {
            IDLE_PRIORITY_CLASS => "idle",
            BELOW_NORMAL_PRIORITY_CLASS => "below normal",
            NORMAL_PRIORITY_CLASS => "normal",
            ABOVE_NORMAL_PRIORITY_CLASS => "above normal",
            HIGH_PRIORITY_CLASS => "high",
            REALTIME_PRIORITY_CLASS => "realtime",
            _ => "unknown",
        };
        Some(ProcState {
            affinity: mask as u64,
            priority: priority.to_string(),
        })
    }

    fn free_bytes(&self, drive: &str) -> Result<u64, String> {
        let root = wide(&format!("{drive}\\"));
        let mut free = 0u64;
        // SAFETY: a NUL-terminated path and one writable u64; the two
        // totals are optional and passed as null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                root.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } != 0;
        if ok {
            Ok(free)
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }

    fn bcd(&self) -> Result<Vec<(String, String)>, String> {
        let out = std::process::Command::new("bcdedit")
            .args(["/enum", "{current}"])
            .output()
            .map_err(|e| format!("bcdedit: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            return Err(format!(
                "bcdedit: {} {}",
                out.status,
                text.trim().lines().next().unwrap_or("")
            ));
        }
        Ok(super::parse_bcdedit(&text))
    }

    fn task(&self, name: &str) -> Result<Option<super::TaskState>, String> {
        let out = std::process::Command::new("schtasks")
            .args(["/query", "/tn", name, "/xml"])
            .output()
            .map_err(|e| format!("schtasks: {e}"))?;
        if out.status.success() {
            return Ok(Some(super::parse_task_xml(&String::from_utf8_lossy(
                &out.stdout,
            ))));
        }
        // schtasks' "not found" text is localized; a task is a file under
        // System32\Tasks, so its absence tells a missing task from a
        // failed query.
        let file = Path::new(r"C:\Windows\System32\Tasks").join(name.trim_start_matches('\\'));
        if !file.exists() {
            return Ok(None);
        }
        Err(format!(
            "schtasks: {} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .lines()
                .next()
                .unwrap_or("")
        ))
    }
}

/// Runs `powercfg <args>`; the error names the command, its status and
/// its first output line.
fn powercfg(args: &[String]) -> Result<(), String> {
    system32_tool("powercfg", args)
}

/// Windows' System32 folder (`%SystemRoot%\System32`).
fn system32() -> Result<String, String> {
    std::env::var("SystemRoot")
        .map(|root| format!(r"{root}\System32"))
        .map_err(|e| format!("SystemRoot: {e}"))
}

/// Runs System32's `<tool>.exe <args>`; the error names the command, its
/// status and its first output line.
fn system32_tool(tool: &str, args: &[impl AsRef<str>]) -> Result<(), String> {
    let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    let cmd = format!("{tool} {}", args.join(" "));
    // System32's tool by its full path: apply runs elevated, and a lookup
    // by name would try the exe's own folder first.
    let dir = system32().map_err(|e| format!("{cmd}: {e}"))?;
    let out = std::process::Command::new(format!(r"{dir}\{tool}.exe"))
        .args(&args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Err(format!(
        "{cmd}: {} {}",
        out.status,
        text.trim().lines().next().unwrap_or("")
    ))
}

/// Whether `path` exists; an error other than "not found" (access denied)
/// is an error, never "absent".
fn file_there(path: &str) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{path}: {e}")),
    }
}

/// Whether the key HKLM\`key` exists.
fn key_exists(key: &str) -> Result<bool, String> {
    let mut h: HKEY = std::ptr::null_mut();
    let k = wide(key);
    // SAFETY: a valid name; the key is closed below when opened.
    let r = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, k.as_ptr(), 0, KEY_READ, &mut h) };
    if r == ERROR_SUCCESS {
        // SAFETY: opened above.
        unsafe { RegCloseKey(h) };
        return Ok(true);
    }
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    Err(format!(r"HKLM\{key}: os error {r}"))
}

/// The scheduled tasks `pattern` names, by their full path: each task is
/// a file under System32\Tasks, so the folder is listed, no process is
/// started (the bridge checks in process, dasio#106).
fn tasks_matching(pattern: &str) -> Result<Vec<String>, String> {
    let folder = absent::task_folder(pattern);
    let dir = Path::new(&system32()?).join("Tasks").join(folder);
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let prefix = if folder.is_empty() {
        r"\".to_string()
    } else {
        format!(r"\{folder}\")
    };
    let mut out = Vec::new();
    for ent in entries {
        let ent = ent.map_err(|e| format!("{}: {e}", dir.display()))?;
        if !ent.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = format!("{prefix}{}", ent.file_name().to_string_lossy());
        if absent::task_matches(pattern, &name) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// The folder names under `dir` (for an uninstaller's version folder).
fn folder_names(dir: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// How long `apply` waits for an uninstaller: a hung or interactive one
/// must not hold the boot-time apply or install.ps1 for good (dasio#128).
const UNINSTALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Removes what `r` names (dasio#128): exactly that service, task or
/// program, nothing else. What is already gone is fine: one removal can
/// take others with it (EdgeUpdate's uninstaller removes its services and
/// tasks), and the plan was made before any ran.
fn remove(r: &Removal) -> Result<(), String> {
    match r {
        Removal::Service(s) => {
            // The parse refuses any other; checked again before an
            // irreversible delete.
            if !absent::deletable(s) {
                return Err(format!(
                    "service {s}: not one dasio deletes ({:?})",
                    absent::DELETABLE_SERVICES
                ));
            }
            if !key_exists(&format!(r"SYSTEM\CurrentControlSet\Services\{s}"))? {
                return Ok(());
            }
            // A stopped or disabled service answers "not started" (1062):
            // the delete is what counts.
            let _ = system32_tool("sc", &["stop", s.as_str()]);
            system32_tool("sc", &["delete", s.as_str()])
        }
        Removal::Task(t) => {
            let file = Path::new(&system32()?)
                .join("Tasks")
                .join(t.trim_start_matches('\\'));
            if !file_there(&file.to_string_lossy())? {
                return Ok(());
            }
            system32_tool("schtasks", &["/delete", "/tn", t.as_str(), "/f"])
        }
        Removal::Exe {
            exe,
            uninstall,
            before,
            windows_caller,
            fallback,
        } => {
            if !file_there(exe)? {
                return Ok(());
            }
            let Some((first, args)) = uninstall.split_first() else {
                return Err(format!("{exe}: no uninstaller given"));
            };
            let prog = absent::resolve_uninstaller(first, folder_names, |p| Path::new(p).is_file())
                .ok_or_else(|| format!("{exe}: its uninstaller {first} was not found"))?;
            let mut ran = String::new();
            // The caller's copy, while it exists; the uninstaller's error
            // waits for its delete.
            let mut caller: Option<String> = None;
            let mut pending: Result<(), String> = Ok(());
            for step in absent::exe_steps(args, before, *windows_caller) {
                match step {
                    absent::ExeStep::Write(w) => {
                        // `plan` takes it from `absent::PRE_UNINSTALL_WRITES`
                        // (key, name and data in code): no profile text
                        // reaches the write.
                        write(w.key, w.name, &Value::String(w.data.to_string()))
                            .map_err(|e| format!("{exe}: before the uninstaller: {e}"))?;
                        println!(
                            "before the uninstaller of {exe}: set {} = {:?} (REG_SZ)",
                            w.path(),
                            w.data
                        );
                    }
                    absent::ExeStep::CopyCaller => {
                        let path = copy_caller().map_err(|e| format!("{exe}: {e}"))?;
                        println!(
                            "the uninstaller of {exe} runs through {path} (a copy of {})",
                            absent::CALLER_SOURCE
                        );
                        caller = Some(path);
                    }
                    absent::ExeStep::Uninstall(args) => {
                        let mut cmd = match &caller {
                            Some(path) => {
                                use std::os::windows::process::CommandExt;
                                let line = absent::caller_command(&prog, &args)?;
                                let mut c = std::process::Command::new(path);
                                c.raw_arg(line);
                                c
                            }
                            None => {
                                let mut c = std::process::Command::new(&prog);
                                c.args(&args);
                                c
                            }
                        };
                        let what = format!("{prog} {}", args.join(" "));
                        match run_bounded(&mut cmd, &what) {
                            Ok(status) => ran = format!("ran {what} ({status})"),
                            Err(e) => pending = Err(e),
                        }
                    }
                    absent::ExeStep::DeleteCaller => {
                        if let Some(path) = caller.take() {
                            match std::fs::remove_file(&path) {
                                Ok(()) => println!("deleted {path}"),
                                Err(e) => {
                                    let e = format!("deleting {path}: {e}");
                                    pending = pending.and(Err(e));
                                }
                            }
                        }
                    }
                    absent::ExeStep::Look => {
                        pending.clone()?;
                        // An uninstaller's exit code says little (some end
                        // non-zero after a good uninstall): the exe gone is
                        // what counts.
                        if !file_there(exe)? {
                            return Ok(());
                        }
                    }
                }
            }
            let left = format!("{ran}: {exe} is still there");
            let Some(fb) = fallback else {
                return Err(left);
            };
            // EdgeUpdate's /uninstall keeps itself while WebView2 is
            // installed (dasio#128, dsg + mbc 16.11.2026).
            println!("fallback for {exe}: {left}");
            run_fallback(fb).map_err(|e| format!("{left}; fallback: {e}"))?;
            if file_there(exe)? {
                return Err(format!("{left}; after the fallback too"));
            }
            Ok(())
        }
    }
}

/// The fallback's steps, each printed: the named processes running from
/// the folder stopped, then the folder deleted. The folder is checked
/// against [`absent::REMOVABLE_DIRS`] again before the delete.
fn run_fallback(fb: &Fallback) -> Result<(), String> {
    for step in fb.steps() {
        match step {
            Step::Stop { name, dir } => stop_inside(&name, &dir)?,
            Step::RemoveDir(dir) => {
                if !absent::removable(&dir) {
                    return Err(format!(
                        "{dir}: not a folder dasio deletes ({:?})",
                        absent::REMOVABLE_DIRS
                    ));
                }
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => println!("fallback: deleted {dir}"),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        println!("fallback: {dir} was already gone")
                    }
                    Err(e) => return Err(format!("deleting {dir}: {e}")),
                }
            }
        }
    }
    Ok(())
}

/// How long a stopped process may take to exit, in 100 ms looks.
const STOP_LOOKS: u32 = 100;

/// Stops every process named `name` whose image lies inside `dir`; one
/// elsewhere, or one whose path cannot be read, is left (and printed: a
/// left one inside the folder makes its delete fail, which says so).
fn stop_inside(name: &str, dir: &str) -> Result<(), String> {
    for pid in find_processes(name)? {
        let image = match image_path(pid) {
            Ok(image) => image,
            Err(e) => {
                // Ended since the snapshot, or not ours to read.
                println!("fallback: left {name} pid {pid}: {e}");
                continue;
            }
        };
        if !absent::inside(dir, &image) {
            println!("fallback: left {name} pid {pid} ({image}): not inside {dir}");
            continue;
        }
        // taskkill turns on the debug privilege a stop of the updater's
        // SYSTEM service process needs; the filter keeps it to this name.
        let pid_text = pid.to_string();
        let filter = format!("IMAGENAME eq {name}");
        system32_tool("taskkill", &["/F", "/PID", &pid_text, "/FI", &filter])
            .map_err(|e| format!("{name} pid {pid} ({image}): not stopped: {e}"))?;
        let mut looks = 0;
        while find_processes(name)?.contains(&pid) {
            looks += 1;
            if looks > STOP_LOOKS {
                return Err(format!(
                    "{name} pid {pid} ({image}): still running {} s after the stop",
                    STOP_LOOKS / 10
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("fallback: stopped {name} pid {pid} ({image})");
    }
    Ok(())
}

/// The full image path of process `pid`.
fn image_path(pid: u32) -> Result<String, String> {
    // SAFETY: a query-only handle, closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(format!(
            "cannot open it ({})",
            std::io::Error::last_os_error()
        ));
    }
    // The longest path Windows gives.
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: a valid handle, a buffer of `len` UTF-16 units.
    let ok =
        unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    let err = std::io::Error::last_os_error();
    // SAFETY: the handle opened above, closed once.
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return Err(format!("its path is unreadable ({err})"));
    }
    Ok(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Copies [`absent::CALLER_SOURCE`] to [`absent::caller_path`] and gives
/// the copy's path. A file already there (not dasio's) is left alone and
/// refused: it is never overwritten.
fn copy_caller() -> Result<String, String> {
    let root = std::env::var("SystemRoot").map_err(|e| format!("SystemRoot: {e}"))?;
    let system_temp = Path::new(&format!(r"{root}\SystemTemp")).is_dir();
    let path = absent::caller_path(&root, system_temp);
    let source = format!(r"{root}\{}", absent::CALLER_SOURCE);
    // create_new: the copy never replaces a file it did not make.
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("the Windows caller {path}: {e}"))?;
    let copied =
        std::fs::File::open(&source).and_then(|mut from| std::io::copy(&mut from, &mut to));
    drop(to);
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&path);
        return Err(format!("copying {source} to {path}: {e}"));
    }
    Ok(path)
}

/// Runs `cmd` (`what` names it) and waits at most [`UNINSTALL_LIMIT`];
/// past it the process is killed and the error says so.
fn run_bounded(
    cmd: &mut std::process::Command,
    what: &str,
) -> Result<std::process::ExitStatus, String> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{what}: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("{what}: {e}"))? {
            return Ok(status);
        }
        if start.elapsed() > UNINSTALL_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{what}: still running after {} s, stopped",
                UNINSTALL_LIMIT.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// An ini file as text (UTF-16 with a BOM or 8-bit).
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let w: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return Ok(String::from_utf16_lossy(&w));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The value `name` under HKLM\`key`.
pub(super) fn read(key: &str, name: &str) -> Result<Option<Value>, String> {
    let (k, n) = (wide(key), wide(name));
    let mut kind = 0u32;
    let mut len = 0u32;
    // SAFETY: valid NUL-terminated strings; a size query (no buffer).
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_ANY,
            &mut kind,
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
        return Err(format!("os error {r}"));
    }
    let mut buf = vec![0u8; len as usize + 2];
    let mut len2 = buf.len() as u32;
    // SAFETY: as above, with a buffer of `len2` bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_ANY,
            &mut kind,
            buf.as_mut_ptr().cast(),
            &mut len2,
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if r != ERROR_SUCCESS {
        return Err(format!("os error {r}"));
    }
    buf.truncate(len2 as usize);
    Ok(Some(match kind {
        REG_DWORD if buf.len() >= 4 => {
            Value::Dword(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
        }
        REG_QWORD if buf.len() >= 8 => Value::Qword(u64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ])),
        REG_MULTI_SZ => {
            let w: Vec<u16> = buf
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            Value::MultiSz(super::multi_sz_from_wide(&w))
        }
        REG_SZ | REG_EXPAND_SZ => {
            let w: Vec<u16> = buf
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            Value::String(from_wide(&w))
        }
        _ => Value::Binary(buf),
    }))
}

fn write(key: &str, name: &str, v: &Value) -> Result<(), String> {
    let (k, n) = (wide(key), wide(name));
    let (kind, data): (u32, Vec<u8>) = match v {
        Value::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
        Value::Qword(q) => (REG_QWORD, q.to_le_bytes().to_vec()),
        Value::String(s) => (
            REG_SZ,
            wide(s).iter().flat_map(|c| c.to_le_bytes()).collect(),
        ),
        Value::MultiSz(m) => (
            REG_MULTI_SZ,
            super::multi_sz_to_wide(m)
                .iter()
                .flat_map(|c| c.to_le_bytes())
                .collect(),
        ),
        Value::Binary(b) => (REG_BINARY, b.clone()),
    };
    // SAFETY: valid strings and `data.len()` bytes; the key is created if
    // missing.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            n.as_ptr(),
            kind,
            data.as_ptr().cast(),
            data.len() as u32,
        )
    };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(r"HKLM\{key}\{name}: os error {r}"))
    }
}

/// The net class subkey whose NetCfgInstanceId is `guid`.
fn find_nic_class(guid: &str) -> Option<String> {
    let mut h: HKEY = std::ptr::null_mut();
    let k = wide(NET_CLASS);
    // SAFETY: a valid name; the key is closed below.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, k.as_ptr(), 0, KEY_READ, &mut h) }
        != ERROR_SUCCESS
    {
        return None;
    }
    let mut found = None;
    for i in 0.. {
        let mut name = [0u16; 64];
        let mut len = name.len() as u32;
        // SAFETY: an open key and a buffer of `len` chars.
        let r = unsafe {
            RegEnumKeyExW(
                h,
                i,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if r != ERROR_SUCCESS {
            break;
        }
        let sub = format!(r"{NET_CLASS}\{}", from_wide(&name));
        if let Ok(Some(Value::String(id))) = read(&sub, "NetCfgInstanceId")
            && id.eq_ignore_ascii_case(guid)
        {
            found = Some(sub);
            break;
        }
    }
    // SAFETY: opened above.
    unsafe { RegCloseKey(h) };
    found
}

/// The pid of the first process with image `exe` (case ignored).
fn find_process(exe: &str) -> Option<u32> {
    find_processes(exe).ok()?.into_iter().next()
}

/// The ids of every process whose exe file name is `exe` (case ignored).
fn find_processes(exe: &str) -> Result<Vec<u32>, String> {
    // SAFETY: a process snapshot, closed below.
    let snap: HANDLE = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(format!(
            "the process list is unreadable ({})",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: a zeroed entry with its size set, as the API wants.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut pids = Vec::new();
    // SAFETY: a valid snapshot and entry.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        if from_wide(&e.szExeFile).eq_ignore_ascii_case(exe) {
            pids.push(e.th32ProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot opened above.
    unsafe { CloseHandle(snap) };
    Ok(pids)
}
