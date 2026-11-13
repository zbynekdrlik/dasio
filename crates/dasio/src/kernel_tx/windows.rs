//! The filter's control device: opened once at start, one
//! `DeviceIoControl` per TX tick.

use std::io;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

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

use dstack::media::bypass::FrameSender;

use super::{
    DEVICE_PATH, HEADER, IOCTL_DASIO_KTX_STATUS, IOCTL_DASIO_TX_SEND, MAX_FRAME,
    MAX_STATUS_RECORDS, Relookup, StatusWatch, mark, pack, parse_status,
};

/// Win32 `ERROR_NOT_FOUND`: the filter's STATUS_NOT_FOUND, no bound
/// adapter with the input's IfIndex.
const ERROR_NOT_FOUND: i32 = 1168;

const STATE_BOUND: u8 = 0;
const STATE_UNBOUND: u8 = 1;
const STATE_NO_ADAPTER: u8 = 2;

/// How often the adapter thread reads the filter's media-TX counters.
const STATUS_EVERY: Duration = Duration::from_secs(10);
/// A clean window is logged once a minute; a window with frames on the
/// socket or lost logs at once.
const STATUS_QUIET_EVERY: u32 = 6;

/// The adapter kernel TX sends on. The TX thread only reads the IfIndex
/// and flags a miss; the `kernel-tx-adapter` thread looks the adapter up
/// again, logs each change of state once, and reads the filter's counters
/// for it (dasio#27 A124).
struct Adapter {
    /// The device handle (`KernelTx` closes it only after this thread
    /// ended).
    handle: usize,
    ip: Ipv4Addr,
    if_index: AtomicU32,
    lost: AtomicBool,
    stop: AtomicBool,
    state: AtomicU8,
}

impl Adapter {
    fn watch(&self) {
        let since = Instant::now();
        let mut gate = Relookup::default();
        let mut counters = StatusWatch::default();
        let mut next_status = Instant::now() + STATUS_EVERY;
        let mut quiet: u32 = 0;
        while !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            if Instant::now() >= next_status {
                next_status += STATUS_EVERY;
                self.counters(&mut counters, &mut quiet);
            }
            if !self.lost.load(Ordering::Relaxed) {
                continue;
            }
            if !gate.due(since.elapsed().as_millis() as u64) {
                continue;
            }
            self.lost.store(false, Ordering::Relaxed);
            let old = self.if_index.load(Ordering::Relaxed);
            let state = match if_index_of(self.ip) {
                Ok(new) if new != old => {
                    self.if_index.store(new, Ordering::Relaxed);
                    info!(old, new, ip = %self.ip, "kernel TX: the adapter came back with a new IfIndex");
                    STATE_BOUND
                }
                Ok(_) => STATE_UNBOUND,
                Err(_) => STATE_NO_ADAPTER,
            };
            if self.state.swap(state, Ordering::Relaxed) != state {
                match state {
                    STATE_UNBOUND => warn!(
                        if_index = old,
                        ip = %self.ip,
                        "kernel TX: the filter is not bound to the adapter: audio TX is on the UDP sockets"
                    ),
                    STATE_NO_ADAPTER => warn!(
                        ip = %self.ip,
                        "kernel TX: no adapter holds the device's address: audio TX is on the UDP sockets"
                    ),
                    _ => {}
                }
            }
        }
    }

    /// Reads the filter's status and logs this adapter's media-TX window.
    fn counters(&self, watch: &mut StatusWatch, quiet: &mut u32) {
        let mut reply = vec![0u8; MAX_STATUS_RECORDS * 27 * 8];
        let mut returned: u32 = 0;
        // SAFETY: the handle stays open while this thread runs (Drop joins
        // it first); the output buffer outlives the synchronous call.
        let ok = unsafe {
            DeviceIoControl(
                self.handle as HANDLE,
                IOCTL_DASIO_KTX_STATUS,
                std::ptr::null(),
                0,
                reply.as_mut_ptr().cast(),
                reply.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let e = io::Error::last_os_error();
            debug!(error = %e, "kernel TX: the filter's status could not be read");
            return;
        }
        reply.truncate(returned as usize);
        let if_index = self.if_index.load(Ordering::Relaxed);
        let Some(r) = watch.step(&parse_status(&reply), if_index) else {
            return;
        };
        if r.bad() {
            *quiet = 0;
            warn!(
                if_index,
                frames = r.frames,
                no_slot = r.no_slot,
                failed = r.failed,
                fail_events = r.fail_events,
                no_slot_events = r.no_slot_events,
                last_no_slot_ms_ago = ?r.last_no_slot_ago_ms,
                last_fail_ms_ago = ?r.last_fail_ago_ms,
                max_out = r.max_out,
                tx_max_out = r.tx_max_out,
                last_fail_status = format_args!("{:#x}", r.last_fail_status),
                "kernel TX: frames went to the socket (no_slot) or the NIC refused them (failed) in the last 10 s"
            );
            return;
        }
        *quiet += 1;
        if *quiet >= STATUS_QUIET_EVERY {
            *quiet = 0;
            info!(
                if_index,
                frames = r.frames,
                max_out = r.max_out,
                tx_max_out = r.tx_max_out,
                out = r.out,
                "kernel TX: the filter took every frame, none failed (last 10 s)"
            );
        }
    }
}

/// Frames one batch carries at most: a TX tick sends one packet per slot.
pub const MAX_BATCH: usize = 64;

/// The open device and the adapter it sends on.
pub struct KernelTx {
    handle: HANDLE,
    adapter: Arc<Adapter>,
    /// The adapter thread, joined before the handle closes.
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
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
        let adapter = Arc::new(Adapter {
            handle: handle as usize,
            ip,
            if_index: AtomicU32::new(if_index),
            lost: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            state: AtomicU8::new(STATE_BOUND),
        });
        let watcher = Arc::clone(&adapter);
        let thread = match std::thread::Builder::new()
            .name("kernel-tx-adapter".into())
            .spawn(move || watcher.watch())
        {
            Ok(t) => t,
            Err(e) => {
                // SAFETY: opened just above, closed once.
                unsafe { CloseHandle(handle) };
                return Err(e);
            }
        };
        Ok(KernelTx {
            handle,
            adapter,
            thread: Mutex::new(Some(thread)),
            buf: Mutex::new(Vec::with_capacity(HEADER + MAX_BATCH * (2 + MAX_FRAME))),
        })
    }

    /// The adapter's IfIndex.
    pub fn if_index(&self) -> u32 {
        self.adapter.if_index.load(Ordering::Relaxed)
    }

    /// Sends `frames` and returns how many the filter took, in order.
    pub fn send_batch(&self, frames: &[&[u8]]) -> io::Result<u32> {
        let Ok(mut buf) = self.buf.try_lock() else {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        };
        pack(frames, self.if_index(), &mut buf)
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
            let e = io::Error::last_os_error();
            drop(buf);
            if e.raw_os_error() == Some(ERROR_NOT_FOUND) {
                // Looked up off the TX thread (it must not block).
                self.adapter.lost.store(true, Ordering::Relaxed);
            }
            return Err(e);
        }
        // Bound again: a later loss warns again.
        if self.adapter.state.load(Ordering::Relaxed) != STATE_BOUND {
            self.adapter.state.store(STATE_BOUND, Ordering::Relaxed);
        }
        Ok(accepted)
    }
}

impl FrameSender for KernelTx {
    fn send(&self, frames: &[&[u8]], sent: &mut [bool]) -> io::Result<()> {
        let accepted = self.send_batch(frames)?;
        mark(accepted, sent);
        Ok(())
    }
}

impl Drop for KernelTx {
    fn drop(&mut self) {
        self.adapter.stop.store(true, Ordering::Relaxed);
        // The thread reads the status on this handle: let it end first
        // (it checks `stop` every 100 ms).
        if let Some(t) = self.thread.get_mut().ok().and_then(Option::take) {
            let _ = t.join();
        }
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
