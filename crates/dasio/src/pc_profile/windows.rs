//! The PC as the profile sees it: HKLM values (with the Dante NIC's keys
//! resolved), ini lines, running processes and free disk space; and the
//! writes `apply` makes. No process is spawned.

use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ,
    RRF_RT_ANY, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, REALTIME_PRIORITY_CLASS,
};

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

/// The PC, with the Dante NIC's keys found from the profile's GUID.
pub struct Pc {
    nic_class: Option<String>,
    nic_device: Option<String>,
}

impl Pc {
    pub fn new(p: &Profile) -> Pc {
        let nic_class = p.nic_guid.as_deref().and_then(find_nic_class);
        let nic_device = nic_class
            .as_deref()
            .and_then(|k| match read(k, "DeviceInstanceID") {
                Ok(Some(Value::String(id))) => Some(format!(
                    r"SYSTEM\CurrentControlSet\Enum\{id}\Device Parameters"
                )),
                _ => None,
            });
        Pc {
            nic_class,
            nic_device,
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
        }
    }
}

impl Reader for Pc {
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
fn read(key: &str, name: &str) -> Result<Option<Value>, String> {
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
    // SAFETY: a process snapshot, closed below.
    let snap: HANDLE = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: a zeroed entry with its size set, as the API wants.
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut pid = None;
    // SAFETY: a valid snapshot and entry.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        if from_wide(&e.szExeFile).eq_ignore_ascii_case(exe) {
            pid = Some(e.th32ProcessID);
            break;
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: the snapshot opened above.
    unsafe { CloseHandle(snap) };
    pid
}
