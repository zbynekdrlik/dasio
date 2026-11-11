//! What the host tests share: the driver as COM makes it, its methods,
//! and a stand-in device that plays blocks.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use dasio_driver::ident::clsid;
use shm::Shared;

/// The driver for instance `n`, as COM makes it.
pub fn driver(n: u32) -> *mut c_void {
    let id = clsid(n).unwrap();
    let mut factory: *mut c_void = core::ptr::null_mut();
    // SAFETY: valid pointers.
    let hr = unsafe { dasio_driver::DllGetClassObject(&id, &IID_ICLASSFACTORY, &mut factory) };
    assert_eq!(hr, S_OK);
    // SAFETY: the factory just made.
    let fv = unsafe { &**(factory as *const *const IClassFactoryVtbl) };
    let mut d: *mut c_void = core::ptr::null_mut();
    // SAFETY: as a host calls it: the CLSID as the interface id.
    assert_eq!(
        unsafe { (fv.CreateInstance)(factory, core::ptr::null_mut(), &id, &mut d) },
        S_OK
    );
    // SAFETY: the factory's last reference.
    unsafe { (fv.Release)(factory) };
    d
}

/// The driver's IASIO methods.
pub fn vt(d: *mut c_void) -> &'static IAsioVtbl {
    // SAFETY: a live driver.
    unsafe { &**(d as *const *const IAsioVtbl) }
}

/// A stand-in device (a 64-sample block): it beats and publishes a block of
/// a non-zero input on channel 0 every ms until `run` is cleared.
pub fn play(device: &Shared, run: &AtomicBool, from_seq: u64) -> u64 {
    let mut seq = from_seq;
    let started = Instant::now();
    while run.load(Ordering::Acquire) {
        seq += 1;
        let half = (seq % 2) as u32;
        let input = device.buffer(half, true, 0).unwrap();
        for k in 0..64 {
            // SAFETY: 64 samples of the instance's input buffer.
            unsafe { input.add(k).write(0x7f00) };
        }
        device
            .header()
            .heartbeat_ms
            .store(started.elapsed().as_millis() as u64 + 1, Ordering::Release);
        device.header().seq.store(seq, Ordering::Release);
        device.signal_block();
        std::thread::sleep(Duration::from_millis(1));
    }
    seq
}
