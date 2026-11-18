//! A process's I/O, memory and GPU priority and its thread priority boost
//! (dasio#185: what Process Lasso set on FOH beyond affinity and priority
//! class), read for the profile check and the keeper and set by the
//! keeper. ntdll's I/O priority calls and gdi32's GPU calls are looked up
//! at run time; gdi32 is loaded only for an entry naming `gpu_priority`,
//! so a media process (a bridge checks its profile in process) never loads
//! it.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::{
    GetProcessInformation, GetProcessPriorityBoost, MEMORY_PRIORITY_INFORMATION,
    ProcessMemoryPriority, SetProcessInformation, SetProcessPriorityBoost,
};

use super::super::keep::Set;
use super::super::{
    Extras, GPU_PRIORITIES, IO_PRIORITIES, MEMORY_PRIORITIES, ProcEntry, boost_name, level_name,
    level_value,
};
use super::wide;

/// ntdll's `PROCESSINFOCLASS` ProcessIoPriority.
const PROCESS_IO_PRIORITY: u32 = 33;

type NtQuery = unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32, *mut u32) -> i32;
type NtSet = unsafe extern "system" fn(HANDLE, u32, *const c_void, u32) -> i32;
type GpuGet = unsafe extern "system" fn(HANDLE, *mut i32) -> i32;
type GpuSet = unsafe extern "system" fn(HANDLE, i32) -> i32;

/// The address of `name` in `module` (NUL-terminated ASCII), if any.
fn lookup(module: *mut c_void, name: &[u8]) -> Option<usize> {
    if module.is_null() {
        return None;
    }
    // SAFETY: a loaded module and a NUL-terminated name.
    unsafe { GetProcAddress(module, name.as_ptr()) }.map(|f| f as usize)
}

/// ntdll's I/O priority query and set (ntdll is in every process).
fn ntdll() -> Option<(NtQuery, NtSet)> {
    static FNS: OnceLock<Option<(usize, usize)>> = OnceLock::new();
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
            std::mem::transmute::<usize, NtQuery>(q),
            std::mem::transmute::<usize, NtSet>(s),
        )
    })
}

/// gdi32's GPU scheduling priority get and set, gdi32 loaded from
/// System32 on first use (and kept: one load per process).
fn gdi32() -> Option<(GpuGet, GpuSet)> {
    static FNS: OnceLock<Option<(usize, usize)>> = OnceLock::new();
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
            std::mem::transmute::<usize, GpuGet>(g),
            std::mem::transmute::<usize, GpuSet>(s),
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

/// Whether `e` names anything read here (its handle then needs
/// PROCESS_QUERY_INFORMATION).
pub(super) fn named(e: &ProcEntry) -> bool {
    e.io_priority.is_some()
        || e.memory_priority.is_some()
        || e.gpu_priority.is_some()
        || e.priority_boost.is_some()
}

/// What `e` names, read through `h`.
pub(super) fn read(h: HANDLE, e: &ProcEntry) -> Extras {
    let name = |v: Option<u32>, names: &[&'static str], first: u32| {
        v.and_then(|v| level_name(names, first, v))
            .map(String::from)
    };
    Extras {
        io: e
            .io_priority
            .as_ref()
            .and_then(|_| name(io(h), &IO_PRIORITIES, 0)),
        memory: e
            .memory_priority
            .as_ref()
            .and_then(|_| name(memory(h), &MEMORY_PRIORITIES, 1)),
        gpu: e
            .gpu_priority
            .as_ref()
            .and_then(|_| name(gpu(h), &GPU_PRIORITIES, 0)),
        boost: e.priority_boost.and_then(|_| boost(h)),
    }
}

/// Sets `want`'s I/O, memory and GPU priority and boost through `h`, each
/// read back, into `done`; the first failure stops it and says why.
pub(super) fn set(h: HANDLE, want: &Set, done: &mut Set) -> Option<String> {
    type Setter = fn(HANDLE, u32) -> Result<(), String>;
    type Getter = fn(HANDLE) -> Option<u32>;
    let levels: [(&str, &Option<String>, &[&'static str], u32, Setter, Getter); 3] = [
        ("io priority", &want.io, &IO_PRIORITIES, 0, set_io, io),
        (
            "memory priority",
            &want.memory,
            &MEMORY_PRIORITIES,
            1,
            set_memory,
            memory,
        ),
        ("gpu priority", &want.gpu, &GPU_PRIORITIES, 0, set_gpu, gpu),
    ];
    for (what, v, names, first, setter, getter) in levels {
        let Some(v) = v else { continue };
        let Some(n) = level_value(names, first, v) else {
            return Some(format!("{what} {v:?} is not one of {names:?}"));
        };
        if let Err(e) = setter(h, n) {
            return Some(format!("{what} {v}: {e}"));
        }
        let now = getter(h);
        if now != Some(n) {
            let read = now
                .and_then(|x| level_name(names, first, x))
                .unwrap_or("unreadable");
            return Some(format!("{what} {v} reads back as {read}"));
        }
        match what {
            "io priority" => done.io = Some(v.clone()),
            "memory priority" => done.memory = Some(v.clone()),
            _ => done.gpu = Some(v.clone()),
        }
    }
    if let Some(on) = want.boost {
        if let Err(e) = set_boost(h, on) {
            return Some(format!("priority boost {}: {e}", boost_name(on)));
        }
        if boost(h) != Some(on) {
            return Some(format!(
                "priority boost {} reads back as {}",
                boost_name(on),
                boost(h).map_or("unreadable".into(), boost_name)
            ));
        }
        done.boost = Some(on);
    }
    None
}
