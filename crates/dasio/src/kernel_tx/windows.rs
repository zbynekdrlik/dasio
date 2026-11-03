//! The filter's control device: opened once at start, one
//! `DeviceIoControl` per TX tick.

use std::io;
use std::net::Ipv4Addr;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::{DEVICE_PATH, HEADER, IOCTL_DASIO_TX_SEND, MAX_FRAME, pack};

/// Frames one batch carries at most: a TX tick sends one packet per slot.
pub const MAX_BATCH: usize = 64;

/// The open device and the adapter it sends on.
pub struct KernelTx {
    handle: HANDLE,
    if_index: u32,
    /// The batch's input, sized once ([`MAX_BATCH`] full frames); only
    /// the TX thread sends, so the lock is never contended.
    buf: Mutex<Vec<u8>>,
}

// SAFETY: the handle is a kernel object handle, usable from any thread;
// the buffer is behind a lock.
unsafe impl Send for KernelTx {}
unsafe impl Sync for KernelTx {}

impl KernelTx {
    /// Opens the filter's device for the adapter that holds `ip`.
    pub fn open(ip: Ipv4Addr) -> io::Result<KernelTx> {
        let if_index = if_index_of(ip)?;
        let path: Vec<u16> = DEVICE_PATH.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated path; the result is checked.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(KernelTx {
            handle,
            if_index,
            buf: Mutex::new(Vec::with_capacity(HEADER + MAX_BATCH * (2 + MAX_FRAME))),
        })
    }

    /// The adapter's IfIndex.
    pub fn if_index(&self) -> u32 {
        self.if_index
    }

    /// Sends `frames` and returns how many the filter took, in order.
    pub fn send(&self, frames: &[&[u8]]) -> io::Result<u32> {
        let Ok(mut buf) = self.buf.try_lock() else {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        };
        pack(frames, self.if_index, &mut buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e:?}")))?;
        let mut accepted: u32 = 0;
        let mut returned: u32 = 0;
        // SAFETY: the input and the output outlive the synchronous call,
        // which reads `buf.len()` bytes and writes at most 4.
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_DASIO_TX_SEND,
                buf.as_ptr().cast(),
                buf.len() as u32,
                (&raw mut accepted).cast(),
                std::mem::size_of::<u32>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(accepted)
    }
}

impl Drop for KernelTx {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed once.
        unsafe { CloseHandle(self.handle) };
    }
}

/// The IfIndex of the adapter with the unicast address `ip`.
fn if_index_of(ip: Ipv4Addr) -> io::Result<u32> {
    let mut size: u32 = 16 * 1024;
    loop {
        let mut mem = vec![0u64; (size as usize).div_ceil(8)];
        let first = mem.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `mem` holds `size` bytes, 8-byte aligned; the call
        // writes at most that and says how much it needs otherwise.
        let r = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET),
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                first,
                &mut size,
            )
        };
        const ERROR_BUFFER_OVERFLOW: u32 = 111;
        if r == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        let mut a = first;
        while !a.is_null() {
            // SAFETY: a list the call built inside `mem`, which lives on.
            let ad = unsafe { &*a };
            let mut u = ad.FirstUnicastAddress;
            while !u.is_null() {
                // SAFETY: as above.
                let ua = unsafe { &*u };
                let sa = ua.Address.lpSockaddr;
                // SAFETY: an AF_INET address is a SOCKADDR_IN.
                if !sa.is_null() && unsafe { (*sa).sa_family } == AF_INET {
                    let sin = unsafe { &*sa.cast::<SOCKADDR_IN>() };
                    // SAFETY: the union's IPv4 view.
                    let addr = Ipv4Addr::from(u32::from_be(unsafe { sin.sin_addr.S_un.S_addr }));
                    if addr == ip {
                        // SAFETY: the IfIndex view of the anonymous union.
                        return Ok(unsafe { ad.Anonymous1.Anonymous.IfIndex });
                    }
                }
                u = ua.Next;
            }
            a = ad.Next;
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no adapter holds {ip}"),
        ));
    }
}
