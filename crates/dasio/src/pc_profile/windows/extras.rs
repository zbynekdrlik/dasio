//! A process's I/O, memory and GPU priority and its thread priority boost
//! (dasio#185: what Process Lasso set on FOH beyond affinity and priority
//! class), read for the profile check and the keeper and set by the
//! keeper. ntdll's I/O priority calls and gdi32's GPU calls are looked up
//! at run time; gdi32 is loaded only for an entry naming `gpu_priority`,
//! which a bridge's in-process check leaves out
//! ([`in_process`](super::super::in_process)), so no media process loads
//! it. Each runs through a handle of its own ([`open`]): a process that
//! refuses those rights still has its affinity and priority kept.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::{
    GetProcessInformation, GetProcessPriorityBoost, MEMORY_PRIORITY_INFORMATION, OpenProcess,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    ProcessMemoryPriority, SetProcessInformation, SetProcessPriorityBoost,
};

use super::super::keep::Set;
use super::super::{
    ExtraAccess, Extras, GPU_PRIORITIES, IO_READ, MEMORY_PRIORITIES, ProcEntry, boost_name,
    level_name, level_value,
};
use super::wide;

/// ntdll's `PROCESSINFOCLASS` ProcessIoPriority.
const PROCESS_IO_PRIORITY: u32 = 33;

type NtQuery = unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32, *mut u32) -> i32;
type NtSet = unsafe extern "system" fn(HANDLE, u32, *const c_void, u32) -> i32;
type GpuGet = unsafe extern "system" fn(HANDLE, *mut i32) -> i32;
type GpuSet = unsafe extern "system" fn(HANDLE, i32) -> i32;
/// A looked-up export, before it is given its own signature.
type Export = unsafe extern "system" fn() -> isize;

/// The address of `name` in `module` (NUL-terminated ASCII), if any.
fn lookup(module: *mut c_void, name: &[u8]) -> Option<Export> {
    if module.is_null() {
        return None;
    }
    // SAFETY: a loaded module and a NUL-terminated name.
    unsafe { GetProcAddress(module, name.as_ptr()) }
}

/// ntdll's I/O priority query and set (ntdll is in every process).
fn ntdll() -> Option<(NtQuery, NtSet)> {
    static FNS: OnceLock<Option<(Export, Export)>> = OnceLock::new();
    let (q, s) = (*FNS.get_or_init(|| {
        let name = wide("ntdll.dll");
        // SAFETY: a NUL-terminated name; ntdll is always loaded.
        let m = unsafe { GetModuleHandleW(name.as_ptr()) };
        Some((
            lookup(m, b"NtQueryInformationProcess\0")?,
            lookup(m, b"NtSetInformationProcess\0")?,
        ))
    }))?;
    // SAFETY: the addresses of these two ntdll exports, with their
    // documented signatures.
    Some(unsafe {
        (
            std::mem::transmute::<Export, NtQuery>(q),
            std::mem::transmute::<Export, NtSet>(s),
        )
    })
}

/// gdi32's GPU scheduling priority get and set, gdi32 loaded from
/// System32 on first use (and kept: one load per process).
fn gdi32() -> Option<(GpuGet, GpuSet)> {
    static FNS: OnceLock<Option<(Export, Export)>> = OnceLock::new();
    let (g, s) = (*FNS.get_or_init(|| {
        let name = wide("gdi32.dll");
        // SAFETY: a NUL-terminated name, searched in System32 only.
        let m = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        Some((
            lookup(m, b"D3DKMTGetProcessSchedulingPriorityClass\0")?,
            lookup(m, b"D3DKMTSetProcessSchedulingPriorityClass\0")?,
        ))
    }))?;
    // SAFETY: the addresses of these two gdi32 exports, with their
    // documented signatures.
    Some(unsafe {
        (
            std::mem::transmute::<Export, GpuGet>(g),
            std::mem::transmute::<Export, GpuSet>(s),
        )
    })
}

fn io(h: HANDLE) -> Option<u32> {
    let (query, _) = ntdll()?;
    let mut v = 0u32;
    // SAFETY: a process handle and a 4-byte buffer.
    let st = unsafe {
        query(
            h,
            PROCESS_IO_PRIORITY,
            (&raw mut v).cast(),
            4,
            std::ptr::null_mut(),
        )
    };
    (st >= 0).then_some(v)
}

fn set_io(h: HANDLE, v: u32) -> Result<(), String> {
    let (_, set) = ntdll().ok_or("ntdll's NtSetInformationProcess not found")?;
    // SAFETY: a process handle with PROCESS_SET_INFORMATION and 4 bytes.
    let st = unsafe { set(h, PROCESS_IO_PRIORITY, (&raw const v).cast(), 4) };
    if st >= 0 {
        Ok(())
    } else {
        Err(format!("NTSTATUS 0x{:08X}", st as u32))
    }
}

fn memory(h: HANDLE) -> Option<u32> {
    let mut m = MEMORY_PRIORITY_INFORMATION { MemoryPriority: 0 };
    let size = std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32;
    // SAFETY: a process handle and a buffer of `size` bytes.
    let ok =
        unsafe { GetProcessInformation(h, ProcessMemoryPriority, (&raw mut m).cast(), size) } != 0;
    ok.then_some(m.MemoryPriority)
}

fn set_memory(h: HANDLE, v: u32) -> Result<(), String> {
    let m = MEMORY_PRIORITY_INFORMATION { MemoryPriority: v };
    let size = std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32;
    // SAFETY: a process handle and a buffer of `size` bytes.
    let ok =
        unsafe { SetProcessInformation(h, ProcessMemoryPriority, (&raw const m).cast(), size) }
            != 0;
    ok.then_some(())
        .ok_or_else(|| std::io::Error::last_os_error().to_string())
}

fn gpu(h: HANDLE) -> Option<u32> {
    let (get, _) = gdi32()?;
    let mut v = 0i32;
    // SAFETY: a process handle and a writable class.
    let st = unsafe { get(h, &mut v) };
    (st >= 0).then_some(v as u32)
}

fn set_gpu(h: HANDLE, v: u32) -> Result<(), String> {
    let (_, set) = gdi32().ok_or("gdi32's D3DKMTSetProcessSchedulingPriorityClass not found")?;
    // SAFETY: a process handle and a class value 0-5.
    let st = unsafe { set(h, v as i32) };
    if st >= 0 {
        Ok(())
    } else {
        Err(format!("NTSTATUS 0x{:08X}", st as u32))
    }
}

/// Whether Windows' dynamic priority boost is on.
fn boost(h: HANDLE) -> Option<bool> {
    let mut disabled = 0;
    // SAFETY: a process handle and a writable BOOL.
    let ok = unsafe { GetProcessPriorityBoost(h, &mut disabled) } != 0;
    ok.then_some(disabled == 0)
}

fn set_boost(h: HANDLE, on: bool) -> Result<(), String> {
    // SAFETY: a process handle with PROCESS_SET_INFORMATION.
    let ok = unsafe { SetProcessPriorityBoost(h, i32::from(!on)) } != 0;
    ok.then_some(())
        .ok_or_else(|| std::io::Error::last_os_error().to_string())
}

/// A handle to `pid` for what `e` names (closed on drop); `None` when `e`
/// names nothing here or the process refuses the rights (then each reads
/// as unreadable). `set` adds the set right the keeper needs.
pub(super) struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle `open` returned, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn open(pid: u32, e: &ProcEntry, set: bool) -> Option<Handle> {
    let mut rights = match e.extra_access() {
        ExtraAccess::None => return None,
        ExtraAccess::Query => PROCESS_QUERY_INFORMATION,
        ExtraAccess::QueryAndSet => PROCESS_QUERY_INFORMATION | PROCESS_SET_INFORMATION,
    };
    if set {
        rights |= PROCESS_SET_INFORMATION;
    }
    // SAFETY: plain arguments; the handle is closed by Drop.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | rights, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

/// One priority kind: its names by value from `first`, read and set
/// through a process handle.
struct Level {
    what: &'static str,
    names: &'static [&'static str],
    first: u32,
    get: fn(HANDLE) -> Option<u32>,
    put: fn(HANDLE, u32) -> Result<(), String>,
}

const IO: Level = Level {
    what: "io priority",
    names: &IO_READ,
    first: 0,
    get: io,
    put: set_io,
};

const MEMORY: Level = Level {
    what: "memory priority",
    names: &MEMORY_PRIORITIES,
    first: 1,
    get: memory,
    put: set_memory,
};

const GPU: Level = Level {
    what: "gpu priority",
    names: &GPU_PRIORITIES,
    first: 0,
    get: gpu,
    put: set_gpu,
};

impl Level {
    /// Its name now, `None` when unreadable.
    fn read(&self, h: HANDLE) -> Option<String> {
        (self.get)(h)
            .and_then(|v| level_name(self.names, self.first, v))
            .map(String::from)
    }

    /// Sets `v` and reads it back; the error says why it did not hold.
    fn apply(&self, h: HANDLE, v: &str) -> Result<(), String> {
        let what = self.what;
        let Some(n) = level_value(self.names, self.first, v) else {
            return Err(format!("{what} {v:?} is not one of {:?}", self.names));
        };
        (self.put)(h, n).map_err(|e| format!("{what} {v}: {e}"))?;
        let now = self.read(h);
        if now.as_deref() != Some(v) {
            let now = now.unwrap_or_else(|| "unreadable".into());
            return Err(format!("{what} {v} reads back as {now}"));
        }
        Ok(())
    }
}

/// What `e` names, read through `h` (none: all unreadable).
pub(super) fn read(h: Option<&Handle>, e: &ProcEntry) -> Extras {
    let Some(Handle(h)) = h else {
        return Extras::default();
    };
    let h = *h;
    Extras {
        io: e.io_priority.as_ref().and_then(|_| IO.read(h)),
        memory: e.memory_priority.as_ref().and_then(|_| MEMORY.read(h)),
        gpu: e.gpu_priority.as_ref().and_then(|_| GPU.read(h)),
        boost: e.priority_boost.and_then(|_| boost(h)),
    }
}

/// Sets `want`'s I/O, memory and GPU priority and boost through `h`, each
/// read back, into `done`; the first failure stops it and says why.
pub(super) fn set(h: Option<&Handle>, want: &Set, done: &mut Set) -> Option<String> {
    let wants =
        want.io.is_some() || want.memory.is_some() || want.gpu.is_some() || want.boost.is_some();
    let h = match h {
        Some(Handle(h)) => *h,
        None if wants => return Some("no handle to set it through".into()),
        None => return None,
    };
    for (level, v, slot) in [
        (&IO, &want.io, &mut done.io),
        (&MEMORY, &want.memory, &mut done.memory),
        (&GPU, &want.gpu, &mut done.gpu),
    ] {
        let Some(v) = v else { continue };
        if let Err(e) = level.apply(h, v) {
            return Some(e);
        }
        *slot = Some(v.clone());
    }
    if let Some(on) = want.boost {
        if let Err(e) = set_boost(h, on) {
            return Some(format!("priority boost {}: {e}", boost_name(on)));
        }
        let now = boost(h);
        if now != Some(on) {
            return Some(format!(
                "priority boost {} reads back as {}",
                boost_name(on),
                now.map_or("unreadable".into(), boost_name)
            ));
        }
        done.boost = Some(on);
    }
    None
}
