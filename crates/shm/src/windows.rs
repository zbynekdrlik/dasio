//! The mapping and the two events on Windows: `Local\dasio-<n>-shm`,
//! `Local\dasio-<n>-block` (the device signals a published block) and
//! `Local\dasio-<n>-ready` (the driver signals returned outputs), both
//! auto-reset. `Local\`: the device process and the DAW run in the same
//! desktop session, though not always as the same user (the FOH PC's master
//! Ableton has its own user, the device runs elevated), so the device gives
//! the objects a DACL for any signed-in user.

use std::io;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0,
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

/// One instance's mapping and events, from either side.
pub struct Shared {
    view: NonNull<u8>,
    shape: Shape,
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
    /// The device side: makes instance `instance`'s mapping for `shape`
    /// (zeroed by the system, or the existing one a driver still holds, whose
    /// session counters are then reset) with its header (`latency`: the DAW's input
    /// and output latency in samples), and its events.
    pub fn create(
        instance: u32,
        shape: Shape,
        rate: u32,
        latency: (u32, u32),
    ) -> io::Result<Shared> {
        let size = shape.size() as u64;
        let security = ObjectSecurity::new()?;
        let attributes = security.attributes();
        // SAFETY: a pagefile-backed mapping with a valid NUL-terminated name;
        // `attributes` outlives the call.
        // SAFETY: as above; GetLastError right after the call says whether
        // the section existed (a driver still holds it).
        let (mapping, existed) = unsafe {
            let m = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &attributes,
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                name(instance, "shm").as_ptr(),
            );
            (m, GetLastError() == ERROR_ALREADY_EXISTS)
        };
        let mapping = handle(mapping)?;
        // A section cannot be resized: a DAW that holds the old shape keeps
        // it until it closes its audio device.
        if existed
            && let (true, Some(held)) = peek_shape(&mapping)?
            && held != shape
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "instance {instance}: the DAW still holds the shared memory of the \
                             old layout (rx {} tx {} block {}, now rx {} tx {} block {}): \
                             close the DAW's audio device (or the DAW), then this device starts",
                    held.rx, held.tx, held.block, shape.rx, shape.tx, shape.block
                ),
            ));
        }
        let shared = Self::map(mapping, shape, instance, Some(&attributes))?;
        let h = shared.header();
        // A driver that still holds the section (the DAW kept it open across
        // a device restart) gives the old section back, not a zeroed one: the
        // new session's counters start from zero.
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
        Ok(shared)
    }

    /// The driver side: opens instance `instance`'s mapping and events; its
    /// shape from the header. `NotFound` while no device made them.
    pub fn open(instance: u32) -> io::Result<Shared> {
        // SAFETY: a valid NUL-terminated name.
        let mapping = handle(unsafe {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name(instance, "shm").as_ptr())
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
        Self::map(mapping, shape, instance, None)
    }

    /// `create`: the device makes the events with these attributes; `None`:
    /// the driver opens them.
    fn map(
        mapping: Handle,
        shape: Shape,
        instance: u32,
        create: Option<&SECURITY_ATTRIBUTES>,
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
        let block = event("block")?;
        let ready = event("ready")?;
        Ok(Shared {
            view,
            shape,
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
