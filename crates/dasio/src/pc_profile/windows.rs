//! The PC as the profile sees it: HKLM values (with the Dante NIC's keys
//! resolved), ini lines, running processes and free disk space; and the
//! writes `apply` makes (power-plan values through powercfg, dasio#128);
//! what an `[[absent]]` entry names and its removal (dasio#128).

use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, REG_BINARY, REG_DWORD, REG_EXPAND_SZ,
    REG_MULTI_SZ, REG_QWORD, REG_SZ, RRF_RT_ANY, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS,
};

use super::absent::{self, AbsentEntry, Fallback, Removal, Step, Target};
use super::{Action, DevState, ProcState, Profile, Reader, Value, get_ini, set_ini};

mod device;
mod exe;
pub mod keep;

const NET_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}";

/// The integrity level of process `h`'s token (dasio#128): the last
/// sub-authority of its mandatory label's SID, named by
/// [`super::integrity_name`]. `h` needs only
/// `PROCESS_QUERY_LIMITED_INFORMATION`.
fn integrity(h: HANDLE) -> Result<String, String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: a valid process handle and a writable token handle.
    if unsafe { OpenProcessToken(h, TOKEN_QUERY, &mut token) } == 0 {
        return Err(format!(
            "OpenProcessToken: {}",
            std::io::Error::last_os_error()
        ));
    }
    // A label is a pointer and its SID; 64 bytes hold any mandatory label
    // (SID of one sub-authority: 12 bytes), u64 for the pointer's alignment.
    let mut buf = [0u64; 8];
    let mut len = 0u32;
    // SAFETY: a valid token handle and a writable buffer of the size given.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr().cast(),
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        )
    } != 0;
    let err = std::io::Error::last_os_error();
    // SAFETY: the token opened above, closed once.
    unsafe { CloseHandle(token) };
    if !ok {
        return Err(format!("GetTokenInformation: {err}"));
    }
    // SAFETY: the buffer holds a TOKEN_MANDATORY_LABEL whose SID points
    // into the same buffer, still alive here.
    let rid = unsafe {
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err("the label has no sub-authority".into());
        }
        *GetSidSubAuthority(sid, u32::from(count) - 1)
    };
    Ok(super::integrity_name(rid).to_string())
}

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
    /// The Dante NIC's device instance path, then its parents' (empty when
    /// not read: `[[device]]` then disables nothing).
    nic_chain: Vec<String>,
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
            // Walked only for a profile with devices: no entry, nothing to
            // guard (and no warning for a NIC not read).
            nic_chain: match instance.as_deref() {
                Some(id) if !p.device.is_empty() => device::chain(id),
                _ => Vec::new(),
            },
        }
    }

    /// `key` (as the profile writes it) as its root and the path under it:
    /// the placeholders are HKLM keys, a plain key names its root (HKLM, or
    /// SYSTEM's own hive under HKEY_USERS, dasio#191).
    fn resolve(&self, key: &str) -> Result<(HKEY, String), String> {
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
            return Ok(match super::split_root(key)? {
                (super::Root::Machine, rest) => (HKEY_LOCAL_MACHINE, rest.to_string()),
                (super::Root::Users, rest) => (HKEY_USERS, rest.to_string()),
            });
        };
        Ok((HKEY_LOCAL_MACHINE, k))
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
                let (root, k) = self.resolve(key)?;
                write_in(root, &k, name, value)
            }
            Action::Ini { file, key, value } => {
                let path = Path::new(file);
                let text = read_text(path)?;
                std::fs::write(path, set_ini(&text, key, value)).map_err(|e| format!("{file}: {e}"))
            }
            Action::Remove(r) => remove(r),
            Action::Disable(id) => device::disable(id),
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

    fn nic_chain(&self) -> Vec<String> {
        self.nic_chain.clone()
    }

    fn device(&self, id: &str) -> Result<Option<DevState>, String> {
        device::state(id)
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
        let (root, k) = self.resolve(key)?;
        read_in(root, &k, name)
    }

    fn ini(&self, file: &str, key: &str) -> Result<Option<String>, String> {
        let text = read_text(Path::new(file))?;
        Ok(get_ini(&text, key))
    }

    fn process(&self, exe: &str) -> Option<ProcState> {
        proc_state(find_process(exe)?)
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
    system_root().map(|root| format!(r"{root}\System32"))
}

/// Windows' own folder (`%SystemRoot%`).
fn system_root() -> Result<String, String> {
    std::env::var("SystemRoot").map_err(|e| format!("SystemRoot: {e}"))
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
        } => exe::remove(exe, uninstall, before, *windows_caller, fallback.as_ref()),
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
    read_in(HKEY_LOCAL_MACHINE, key, name)
}

/// The root's name in a message.
fn root_name(root: HKEY) -> &'static str {
    if root == HKEY_USERS { "HKU" } else { "HKLM" }
}

/// The value `name` under `root`\`key`.
fn read_in(root: HKEY, key: &str, name: &str) -> Result<Option<Value>, String> {
    let (k, n) = (wide(key), wide(name));
    let mut kind = 0u32;
    let mut len = 0u32;
    // SAFETY: valid NUL-terminated strings; a size query (no buffer).
    let r = unsafe {
        RegGetValueW(
            root,
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
            root,
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

pub(super) fn write(key: &str, name: &str, v: &Value) -> Result<(), String> {
    write_in(HKEY_LOCAL_MACHINE, key, name, v)
}

/// Sets `name` under `root`\`key` (the key is created when missing).
fn write_in(root: HKEY, key: &str, name: &str, v: &Value) -> Result<(), String> {
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
            root,
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
        Err(format!(r"{}\{key}\{name}: os error {r}", root_name(root)))
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

/// The affinity, priority class and integrity of process `pid`; `None`
/// when it cannot be opened for a limited query (or is gone).
pub(super) fn proc_state(pid: u32) -> Option<ProcState> {
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
    let integrity = integrity(h);
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
        integrity,
    })
}

/// The pid of the first process with image `exe` (case ignored).
fn find_process(exe: &str) -> Option<u32> {
    find_processes(exe).ok()?.into_iter().next()
}

/// The ids of every process whose exe file name is `exe` (case ignored).
pub(super) fn find_processes(exe: &str) -> Result<Vec<u32>, String> {
    Ok(snapshot()?
        .into_iter()
        .filter(|(_, n)| n.eq_ignore_ascii_case(exe))
        .map(|(pid, _)| pid)
        .collect())
}

/// Every running process: its id and its exe file name.
pub(super) fn snapshot() -> Result<Vec<(u32, String)>, String> {
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
    let mut procs = Vec::new();
    // SAFETY: a valid snapshot and entry.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        procs.push((e.th32ProcessID, from_wide(&e.szExeFile)));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot opened above.
    unsafe { CloseHandle(snap) };
    Ok(procs)
}
