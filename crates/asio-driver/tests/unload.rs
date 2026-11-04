//! The DLL must stay loaded while a thread it abandoned still runs its code.
//! A test binary of its own: it reads the process-wide object count.
#![cfg(windows)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use dasio_driver::ident::clsid;

static HUNG_IN: AtomicU32 = AtomicU32::new(0);
static LET_GO: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn hang(_half: i32, _direct: ASIOBool) {
    HUNG_IN.store(1, Ordering::Release);
    let t = Instant::now();
    while LET_GO.load(Ordering::Acquire) == 0 && t.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

unsafe extern "C" fn message(_sel: i32, _val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    0
}

fn driver(n: u32) -> *mut c_void {
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

fn vt(d: *mut c_void) -> &'static IAsioVtbl {
    // SAFETY: a live driver.
    unsafe { &**(d as *const *const IAsioVtbl) }
}

// A host whose callback hangs past the stop bound, then releases the driver
// and asks whether the DLL may unload (CoFreeUnusedLibraries). The abandoned
// thread is still inside the host's callback and returns into the DLL's code
// later (PR #4 review, dasio#1).
#[test]
fn the_dll_stays_loaded_while_an_abandoned_worker_runs() {
    let d = driver(9);
    let mut infos: Vec<ASIOBufferInfo> = (0..2)
        .map(|k| ASIOBufferInfo {
            isInput: k,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(hang),
        sampleRateDidChange: None,
        asioMessage: Some(message),
        bufferSwitchTimeInfo: None,
    };
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 32, &cb),
            ASE_OK
        );
        assert_eq!((vt(d).start)(d), ASE_OK);
    }
    let t = Instant::now();
    while HUNG_IN.load(Ordering::Acquire) == 0 && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(HUNG_IN.load(Ordering::Acquire), 1, "the callback runs");
    // SAFETY: as a host: stop, dispose and release while the callback hangs.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
    let can_unload = dasio_driver::DllCanUnloadNow();
    LET_GO.store(1, Ordering::Release);
    assert_eq!(
        can_unload, S_FALSE,
        "the abandoned thread still runs the DLL's code"
    );
}
