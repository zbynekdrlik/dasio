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

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_ALL_ACCESS, OpenEventW, SetEvent, WaitForSingleObject,
};

use crate::{Header, MAGIC, Shape, VERSION};

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
/// whether it carries our magic and version, and its shape.
fn peek_shape(mapping: &Handle) -> io::Result<(bool, Option<Shape>)> {
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
    let shape = Shape::new(
        h.rx.load(Ordering::Relaxed),
        h.tx.load(Ordering::Relaxed),
        h.block.load(Ordering::Relaxed),
    );
    // SAFETY: the view mapped just above.
    unsafe { UnmapViewOfFile(peek) };
    Ok((ok, shape))
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
    _pad: u32,
    session: AtomicU64,
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
                Ok(m) => match peek_shape(&m)? {
                    (true, Some(s)) if s == shape => Some(m),
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
        let (ok, shape) = peek_shape(&mapping)?;
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
        })
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
        // SAFETY: the view this struct mapped, unmapped once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr().cast(),
            })
        };
    }
}
