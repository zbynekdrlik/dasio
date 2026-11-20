//! The mapping and the two events on Windows: `Local\dasio-<n>-shm-<session>`
//! (found through `Local\dasio-<n>-dir`),
//! `Local\dasio-<n>-block` (the device signals a published block) and
//! `Local\dasio-<n>-ready` (the driver signals returned outputs), both
//! auto-reset. `Local\`: the device process and the DAW run in the same
//! desktop session, though not always as the same user (the FOH PC's master
//! Ableton has its own user, the device runs elevated), so the device gives
//! the objects a DACL for any signed-in user.

use std::io;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, GetProcessWorkingSetSizeEx,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MIN_DISABLE, SetProcessWorkingSetSizeEx,
    UnmapViewOfFile, VirtualLock, VirtualUnlock,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_ALL_ACCESS, GetCurrentProcess, OpenEventW, SetEvent, WaitForSingleObject,
};

use crate::{Header, MAGIC, Shape, VERSION, page_span, ticks_to_ns, working_set};

/// Now on the system-wide performance counter, ns: the one clock the
/// driver (in the DAW's process) and the device share, so a time one of
/// them writes into the header is comparable with the other's.
pub fn qpc_ns() -> u64 {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    static FREQ: OnceLock<u64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: a valid out pointer; never fails on XP and later.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1) as u64
    });
    let mut t = 0i64;
    // SAFETY: a valid out pointer; never fails on XP and later.
    unsafe { QueryPerformanceCounter(&mut t) };
    ticks_to_ns(t.max(0) as u64, freq)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// In the global namespace: the device runs as a service in session 0 and
/// the DAW in the user's session; `Local\` names are per session, so a DAW
/// would never see the service's mapping. Creating a global object needs
/// SeCreateGlobalPrivilege (the service's LocalSystem, an elevated
/// process); opening it, only the DACL below.
fn name(instance: u32, what: &str) -> Vec<u16> {
    wide(&format!("Global\\dasio-{instance}-{what}"))
}

/// The header's shape in `mapping`, read through a header-sized view:
/// whether it carries our magic and version, its shape and its rate.
fn peek_shape(mapping: &Handle) -> io::Result<(bool, Option<Shape>, u32)> {
    // SAFETY: a valid mapping handle; the header's bytes.
    let peek = unsafe {
        MapViewOfFile(
            mapping.0,
            FILE_MAP_ALL_ACCESS,
            0,
            0,
            std::mem::size_of::<Header>(),
        )
    };
    if peek.Value.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the view holds a Header the device wrote (or zeros).
    let h = unsafe { &*(peek.Value as *const Header) };
    let ok =
        h.magic.load(Ordering::Acquire) == MAGIC && h.version.load(Ordering::Relaxed) == VERSION;
    let shape = Shape::app(
        h.rx.load(Ordering::Relaxed),
        h.tx.load(Ordering::Relaxed),
        h.block.load(Ordering::Relaxed),
    );
    let rate = h.rate.load(Ordering::Relaxed);
    // SAFETY: the view mapped just above.
    unsafe { UnmapViewOfFile(peek) };
    Ok((ok, shape, rate))
}

/// Full access for Authenticated Users, SYSTEM and Administrators.
const OBJECT_SDDL: &str = "D:(A;;GA;;;AU)(A;;GA;;;SY)(A;;GA;;;BA)";

/// The security descriptor the device makes its objects with.
struct ObjectSecurity(PSECURITY_DESCRIPTOR);

impl ObjectSecurity {
    fn new() -> io::Result<ObjectSecurity> {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a valid NUL-terminated string; `sd` receives a LocalAlloc'd
        // descriptor this struct frees.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(OBJECT_SDDL).as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ObjectSecurity(sd))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for ObjectSecurity {
    fn drop(&mut self) {
        // SAFETY: the descriptor the conversion allocated, freed once.
        unsafe { LocalFree(self.0) };
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle this struct owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn handle(h: HANDLE) -> io::Result<Handle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(h))
    }
}

/// The instance's directory, `Global\dasio-<n>-dir`: which session's
/// section is the device's now. A section cannot be resized, so a device
/// that restarts with another shape while a DAW still holds the old section
/// makes a fresh one (`Global\dasio-<n>-shm-<session>`) and publishes it
/// here; the driver sees a new session and asks the DAW for a reset.
#[repr(C)]
struct Dir {
    magic: AtomicU32,
    /// The DAW's process id while a driver has the buffers, 0 otherwise
    /// (dasio#22: the device names the DAW's threads at a burst). In what
    /// was padding: drivers before 0.1.52 leave it 0.
    host_pid: AtomicU32,
    session: AtomicU64,
    /// The sample rate the DAW asked (`setSampleRate`, dasio#107), Hz; 0
    /// while none asked. The device serves the next session at it. Here,
    /// not in a section's header: the directory outlives the sessions, so
    /// a device restarted under a DAW at 48 kHz comes back at 48 kHz.
    requested_rate: AtomicU32,
    _pad: AtomicU32,
}

const DIR_MAGIC: u32 = u32::from_le_bytes(*b"DSDR");

/// A mapped directory.
struct Directory {
    view: NonNull<Dir>,
    _mapping: Handle,
}

impl Directory {
    fn create(instance: u32, attributes: &SECURITY_ATTRIBUTES) -> io::Result<Directory> {
        // SAFETY: a pagefile-backed mapping with a valid name; `attributes`
        // outlives the call. An existing directory (a driver holds it) is
        // returned as it is.
        let mapping = handle(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                attributes,
                PAGE_READWRITE,
                0,
                std::mem::size_of::<Dir>() as u32,
                name(instance, "dir").as_ptr(),
            )
        })?;
        Self::map(mapping)
    }

    fn open(instance: u32) -> io::Result<Directory> {
        // SAFETY: a valid NUL-terminated name.
        let mapping = handle(unsafe {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name(instance, "dir").as_ptr())
        })?;
        Self::map(mapping)
    }

    fn map(mapping: Handle) -> io::Result<Directory> {
        // SAFETY: a valid mapping handle of at least a Dir.
        let view = unsafe {
            MapViewOfFile(
                mapping.0,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<Dir>(),
            )
        };
        let view = NonNull::new(view.Value.cast::<Dir>()).ok_or_else(io::Error::last_os_error)?;
        Ok(Directory {
            view,
            _mapping: mapping,
        })
    }

    fn dir(&self) -> &Dir {
        // SAFETY: the view holds a Dir (zeroed until a device writes it).
        unsafe { self.view.as_ref() }
    }

    /// The published session; 0 while none is.
    fn session(&self) -> u64 {
        let d = self.dir();
        if d.magic.load(Ordering::Acquire) == DIR_MAGIC {
            d.session.load(Ordering::Acquire)
        } else {
            0
        }
    }

    fn requested_rate(&self) -> u32 {
        self.dir().requested_rate.load(Ordering::Acquire)
    }

    fn request_rate(&self, rate: u32) {
        self.dir().requested_rate.store(rate, Ordering::Release);
    }

    fn publish(&self, session: u64) {
        let d = self.dir();
        d.session.store(session, Ordering::Release);
        d.magic.store(DIR_MAGIC, Ordering::Release);
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: the view this struct mapped, unmapped once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr().cast(),
            })
        };
    }
}

/// The sample rate a DAW asked instance `instance` for (dasio#107), Hz; 0
/// while none asked or no directory exists yet (no driver and no device
/// ever opened it): the device reads it before it makes its section.
pub fn requested_rate(instance: u32) -> io::Result<u32> {
    match Directory::open(instance) {
        Ok(d) => Ok(d.requested_rate()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

/// The driver side without a device's section: the DAW asks for `rate`
/// Hz through the instance's directory, kept for the device's next
/// session. `NotFound` while no directory exists.
pub fn request_rate(instance: u32, rate: u32) -> io::Result<()> {
    Directory::open(instance)?.request_rate(rate);
    Ok(())
}

/// The device's locked views, all sections together (dasio#28 E11): a
/// re-serve locks its new section while the old one still lives.
struct Locks {
    /// The process's working set (minimum, maximum) before its first lock.
    base: Option<(usize, usize)>,
    /// The bytes locked now.
    bytes: usize,
}

static LOCKS: Mutex<Locks> = Mutex::new(Locks {
    base: None,
    bytes: 0,
});

fn page_size() -> usize {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: a valid out pointer; never fails.
    unsafe { GetSystemInfo(&mut info) };
    info.dwPageSize as usize
}

/// The process's working set for `locked` bytes on top of `base`, as soft
/// limits: the system may still grow the process past the maximum or trim
/// it below the minimum under memory pressure; only the minimum's quota
/// for locked pages matters here. A hard limit someone else set on the
/// process (none of dasio's tools sets one) becomes soft too.
fn set_working_set(base: (usize, usize), locked: usize) -> io::Result<()> {
    let (min, max) = working_set(base, locked);
    // SAFETY: the current process's pseudo handle; plain values.
    let ok = unsafe {
        SetProcessWorkingSetSizeEx(
            GetCurrentProcess(),
            min,
            max,
            QUOTA_LIMITS_HARDWS_MIN_DISABLE | QUOTA_LIMITS_HARDWS_MAX_DISABLE,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Why [`Shared::lock`] locked nothing: the step and the OS error.
#[derive(Debug)]
pub struct LockFailed {
    /// "reading the working set", "raising the working set" or "locking a
    /// view".
    pub step: &'static str,
    pub err: io::Error,
}

impl LockFailed {
    fn at(step: &'static str) -> LockFailed {
        LockFailed {
            step,
            err: io::Error::last_os_error(),
        }
    }
}

impl std::fmt::Display for LockFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.step, self.err)
    }
}

/// Sessions this process made (a test makes several devices in one process).
static SESSIONS: AtomicU32 = AtomicU32::new(0);

/// A session id no other device run has: this process and a counter.
fn new_session() -> u64 {
    (u64::from(std::process::id()) << 32) | u64::from(SESSIONS.fetch_add(1, Ordering::Relaxed) + 1)
}

fn section_name(instance: u32, session: u64) -> Vec<u16> {
    name(instance, &format!("shm-{session:016x}"))
}

/// One instance's mapping and events, from either side.
pub struct Shared {
    view: NonNull<u8>,
    shape: Shape,
    session: u64,
    dir: Directory,
    _mapping: Handle,
    block: Handle,
    ready: Handle,
    /// The bytes [`Shared::lock`] locked (0: none), given back at drop.
    locked: usize,
}

// SAFETY: the view is shared memory accessed through atomics (the header)
// and through buffers whose ownership the seq/ack protocol hands over.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Shared {
    /// The device side: instance `instance`'s mapping for `shape` with its
    /// header (`latency`: the DAW's input and output latency in samples),
    /// and its events. A section of the same shape a driver still holds
    /// (the DAW kept it across a device restart) is taken over, its session
    /// counters reset; otherwise a fresh section (zeroed by the system) is
    /// made and published as a new session.
    pub fn create(
        instance: u32,
        shape: Shape,
        rate: u32,
        latency: (u32, u32),
    ) -> io::Result<Shared> {
        let security = ObjectSecurity::new()?;
        let attributes = security.attributes();
        let dir = Directory::create(instance, &attributes)?;
        let held = dir.session();
        let reuse = if held != 0 {
            // SAFETY: a valid NUL-terminated name.
            let m = unsafe {
                OpenFileMappingW(
                    FILE_MAP_ALL_ACCESS,
                    0,
                    section_name(instance, held).as_ptr(),
                )
            };
            match handle(m) {
                // Taken over only at the same shape and rate: a driver
                // holding it sees no new session, so another rate at the
                // same block (44.1 and 48 kHz on a 16-sample device
                // block) must make a fresh one (dasio#107).
                Ok(m) => match peek_shape(&m)? {
                    (true, Some(s), r) if s == shape && r == rate => Some(m),
                    _ => None,
                },
                Err(_) => None,
            }
        } else {
            None
        };
        let (mapping, session, fresh) = match reuse {
            Some(m) => (m, held, false),
            None => {
                let session = new_session();
                let size = shape.size() as u64;
                // SAFETY: a pagefile-backed mapping with a valid name;
                // `attributes` outlives the call.
                let m = handle(unsafe {
                    CreateFileMappingW(
                        INVALID_HANDLE_VALUE,
                        &attributes,
                        PAGE_READWRITE,
                        (size >> 32) as u32,
                        size as u32,
                        section_name(instance, session).as_ptr(),
                    )
                })?;
                (m, session, true)
            }
        };
        let shared = Self::map(mapping, shape, instance, Some(&attributes), dir, session)?;
        let h = shared.header();
        // A taken-over section still holds the last run's counters: the new
        // run's start from zero.
        h.seq.store(0, Ordering::Relaxed);
        h.ack.store(0, Ordering::Relaxed);
        h.late.store(0, Ordering::Relaxed);
        h.missed.store(0, Ordering::Relaxed);
        h.version.store(VERSION, Ordering::Relaxed);
        h.rx.store(shape.rx, Ordering::Relaxed);
        h.tx.store(shape.tx, Ordering::Relaxed);
        h.block.store(shape.block, Ordering::Relaxed);
        h.rate.store(rate, Ordering::Relaxed);
        h.latency_in.store(latency.0, Ordering::Relaxed);
        h.latency_out.store(latency.1, Ordering::Relaxed);
        h.device_pid.store(std::process::id(), Ordering::Relaxed);
        h.magic.store(MAGIC, Ordering::Release);
        if fresh {
            // Only now that the header is whole.
            shared.dir.publish(session);
        }
        Ok(shared)
    }

    /// The driver side: opens the instance's published section and its
    /// events; its shape from the header. `NotFound` while no device made
    /// them.
    pub fn open(instance: u32) -> io::Result<Shared> {
        let dir = Directory::open(instance)?;
        let session = dir.session();
        if session == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no dasio device published its section",
            ));
        }
        // SAFETY: a valid NUL-terminated name.
        let mapping = handle(unsafe {
            OpenFileMappingW(
                FILE_MAP_ALL_ACCESS,
                0,
                section_name(instance, session).as_ptr(),
            )
        })?;
        let (ok, shape, _) = peek_shape(&mapping)?;
        let shape = match (ok, shape) {
            (true, Some(s)) => s,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "not a dasio mapping",
                ));
            }
        };
        Self::map(mapping, shape, instance, None, dir, session)
    }

    /// The session of this section.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// The DAW's process id while a driver has the buffers, 0 otherwise
    /// (or with a driver before 0.1.52).
    pub fn host_pid(&self) -> u32 {
        self.dir.dir().host_pid.load(Ordering::Acquire)
    }

    /// The driver side: `pid` has the buffers now (0: nobody).
    pub fn set_host_pid(&self, pid: u32) {
        self.dir.dir().host_pid.store(pid, Ordering::Release);
    }

    /// The sample rate the DAW asked (0: none), read by the device.
    pub fn requested_rate(&self) -> u32 {
        self.dir.requested_rate()
    }

    /// The driver side: the DAW asks for `rate` Hz; the device serves a
    /// new session at it ([`published_session`](Self::published_session)).
    pub fn request_rate(&self, rate: u32) {
        self.dir.request_rate(rate);
    }

    /// The session the device publishes now: another one than
    /// [`session`](Self::session) means a device restarted on a fresh section
    /// (another shape), and the DAW must reset to reach it.
    pub fn published_session(&self) -> u64 {
        self.dir.session()
    }

    /// `create`: the device makes the events with these attributes; `None`:
    /// the driver opens them.
    fn map(
        mapping: Handle,
        shape: Shape,
        instance: u32,
        create: Option<&SECURITY_ATTRIBUTES>,
        dir: Directory,
        session: u64,
    ) -> io::Result<Shared> {
        // SAFETY: a valid mapping handle of at least `shape.size()` bytes.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, shape.size()) };
        let view = NonNull::new(view.Value.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
        let event = |what: &str| -> io::Result<Handle> {
            let n = name(instance, what);
            handle(match create {
                // SAFETY: an auto-reset, unsignalled event with a valid name.
                Some(a) => unsafe { CreateEventW(a, 0, 0, n.as_ptr()) },
                // SAFETY: a valid name.
                None => unsafe { OpenEventW(EVENT_ALL_ACCESS, 0, n.as_ptr()) },
            })
        };
        let block = event("block");
        let ready = event("ready");
        let (block, ready) = match (block, ready) {
            (Ok(b), Ok(r)) => (b, r),
            (Err(e), _) | (_, Err(e)) => {
                // SAFETY: the view mapped just above.
                unsafe {
                    UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: view.as_ptr().cast(),
                    })
                };
                return Err(e);
            }
        };
        Ok(Shared {
            view,
            shape,
            session,
            dir,
            _mapping: mapping,
            block,
            ready,
            locked: 0,
        })
    }

    /// The views this side keeps: the section and the directory (address,
    /// bytes).
    fn views(&self) -> [(usize, usize); 2] {
        [
            (self.view.as_ptr() as usize, self.shape.size()),
            (self.dir.view.as_ptr() as usize, std::mem::size_of::<Dir>()),
        ]
    }

    /// The device side (dasio#28 E11): locks this section's and the
    /// directory's views in memory, so a page fault never stalls the pump or
    /// the bridge on them. Raises the process's working set first (by the
    /// locked bytes and [`crate::LOCK_MARGIN`] over what it had before its
    /// first lock), then `VirtualLock`s each view. The bytes locked (whole
    /// pages); again on a locked section, the same bytes. An error leaves
    /// nothing of this section locked and the working set at what the other
    /// locked sections need: the device runs on as without a lock. The
    /// locks end with the views (unmapping unlocks).
    pub fn lock(&mut self) -> Result<usize, LockFailed> {
        if self.locked != 0 {
            return Ok(self.locked);
        }
        let page = page_size();
        let views = self.views();
        let bytes: usize = views.iter().map(|&(a, l)| page_span(a, l, page)).sum();
        let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
        let base = match locks.base {
            Some(b) => b,
            None => {
                let (mut min, mut max, mut flags) = (0usize, 0usize, 0u32);
                // SAFETY: the current process's pseudo handle; valid out
                // pointers.
                let ok = unsafe {
                    GetProcessWorkingSetSizeEx(GetCurrentProcess(), &mut min, &mut max, &mut flags)
                };
                if ok == 0 {
                    return Err(LockFailed::at("reading the working set"));
                }
                locks.base = Some((min, max));
                (min, max)
            }
        };
        set_working_set(base, locks.bytes + bytes).map_err(|err| LockFailed {
            step: "raising the working set",
            err,
        })?;
        for (i, &(addr, len)) in views.iter().enumerate() {
            // SAFETY: a range inside a view this struct mapped.
            if unsafe { VirtualLock(addr as *const _, len) } == 0 {
                let e = io::Error::last_os_error();
                for &(addr, len) in &views[..i] {
                    // SAFETY: a range locked just above.
                    unsafe { VirtualUnlock(addr as *const _, len) };
                }
                // Back to what the other locked sections need; the lock's
                // own error is the one to report.
                let _ = set_working_set(base, locks.bytes);
                return Err(LockFailed {
                    step: "locking a view",
                    err: e,
                });
            }
        }
        locks.bytes += bytes;
        self.locked = bytes;
        Ok(bytes)
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn header(&self) -> &Header {
        // SAFETY: the view starts with the Header, aligned (the system maps
        // at a 64 KiB boundary).
        unsafe { &*(self.view.as_ptr() as *const Header) }
    }

    /// One buffer's samples (see [`Shape::buffer_offset`]); `None` past the
    /// shape. The caller owns the half the seq/ack protocol gave it.
    pub fn buffer(&self, half: u32, input: bool, ch: u32) -> Option<*mut i32> {
        let o = self.shape.buffer_offset(half, input, ch)?;
        // SAFETY: inside the view (the offset and the block fit its size).
        Some(unsafe { self.view.as_ptr().add(o) }.cast::<i32>())
    }

    /// The device: a block is published.
    pub fn signal_block(&self) {
        // SAFETY: a valid event handle.
        unsafe { SetEvent(self.block.0) };
    }

    /// The driver: waits up to `ms` for a published block.
    pub fn wait_block(&self, ms: u32) -> bool {
        // SAFETY: a valid event handle.
        unsafe { WaitForSingleObject(self.block.0, ms) == WAIT_OBJECT_0 }
    }

    /// The driver: the DAW's outputs are in.
    pub fn signal_ready(&self) {
        // SAFETY: a valid event handle.
        unsafe { SetEvent(self.ready.0) };
    }

    /// The device: waits up to `ms` for the DAW's outputs.
    pub fn wait_ready(&self, ms: u32) -> bool {
        // SAFETY: a valid event handle.
        unsafe { WaitForSingleObject(self.ready.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the view this struct mapped, unmapped once (which ends
        // its lock; the directory's ends when `dir` drops next).
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr().cast(),
            })
        };
        if self.locked != 0 {
            // Teardown only, on whichever thread drops the last handle: the
            // mutex waits at most for another section's lock. The working
            // set stays raised until the next lock sets it
            // for what is locked then.
            let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
            locks.bytes = locks.bytes.saturating_sub(self.locked);
        }
    }
}
