//! The driver through its COM entry, as a DAW drives it, with a stand-in
//! device on the instance's shared memory (plan Task 6).
#![cfg(windows)]

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use dasio_driver::ident::clsid;
use shm::{Shape, Shared};

#[test]
fn abi_layout_matches_the_sdk_x64() {
    use std::mem::{offset_of, size_of};
    assert_eq!(size_of::<ASIOChannelInfo>(), 52);
    assert_eq!(size_of::<ASIOClockSource>(), 48);
    assert_eq!(size_of::<ASIOBufferInfo>(), 24);
    assert_eq!(offset_of!(ASIOBufferInfo, buffers), 8);
    assert_eq!(size_of::<AsioTimeInfo>(), 48);
    assert_eq!(offset_of!(AsioTimeInfo, sampleRate), 24);
    assert_eq!(size_of::<ASIOTimeCode>(), 84);
    assert_eq!(size_of::<ASIOTime>(), 148);
    assert_eq!(size_of::<IAsioVtbl>(), 24 * 8);
}

/// The driver for instance `n`, as COM makes it.
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

// The host's callbacks (plain C functions): copy each input to its output
// in the half handed over, count the switches.
static SWITCHES: AtomicU32 = AtomicU32::new(0);
static LAST_HALF: AtomicI32 = AtomicI32::new(-1);
static BUFFERS: Mutex<Vec<[usize; 2]>> = Mutex::new(Vec::new());
static BLOCK: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn switch(half: i32, _direct: ASIOBool) {
    let b = BUFFERS.lock().unwrap();
    let n = BLOCK.load(Ordering::Relaxed) as usize;
    // Inputs first, then outputs, one each (the test's 1/1 shape).
    if b.len() == 2 {
        let (i, o) = (
            b[0][half as usize] as *const i32,
            b[1][half as usize] as *mut i32,
        );
        // SAFETY: two buffers of `n` samples.
        unsafe { core::ptr::copy_nonoverlapping(i, o, n) };
    }
    LAST_HALF.store(half, Ordering::Release);
    SWITCHES.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "C" fn message(_sel: i32, _val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    0
}

fn callbacks() -> ASIOCallbacks {
    ASIOCallbacks {
        bufferSwitch: Some(switch),
        sampleRateDidChange: None,
        asioMessage: Some(message),
        bufferSwitchTimeInfo: None,
    }
}

fn create(d: *mut c_void, block: i32) -> ASIOError {
    let mut infos = [
        ASIOBufferInfo {
            isInput: 1,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        },
        ASIOBufferInfo {
            isInput: 0,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        },
    ];
    let cb = callbacks();
    // SAFETY: two buffer infos and the callbacks, as a host passes them.
    let e = unsafe { (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, block, &cb) };
    if e == ASE_OK {
        *BUFFERS.lock().unwrap() = infos
            .iter()
            .map(|i| [i.buffers[0] as usize, i.buffers[1] as usize])
            .collect();
        BLOCK.store(block as u32, Ordering::Relaxed);
    }
    e
}

#[test]
fn a_block_reaches_the_host_and_back_and_the_buffer_size_is_the_instance_s() {
    let shape = Shape::new(1, 1, 64).unwrap();
    let device = Shared::create(5, shape, 96_000, (64 + 24, 40)).unwrap();
    let d = driver(5);
    // SAFETY: the driver's methods as a host calls them.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        let (mut i, mut o) = (0, 0);
        assert_eq!((vt(d).getChannels)(d, &mut i, &mut o), ASE_OK);
        assert_eq!((i, o), (1, 1));
        let (mut a, mut b, mut c, mut g) = (0, 0, 0, 0);
        assert_eq!(
            (vt(d).getBufferSize)(d, &mut a, &mut b, &mut c, &mut g),
            ASE_OK
        );
        assert_eq!((a, b, c, g), (64, 64, 64, 0));
        assert_eq!((vt(d).getLatencies)(d, &mut i, &mut o), ASE_OK);
        assert_eq!((i, o), (88, 40));
        assert_eq!((vt(d).canSampleRate)(d, 48_000.0), ASE_NO_CLOCK);
        assert_eq!((vt(d).canSampleRate)(d, 96_000.0), ASE_OK);
    }
    assert_eq!(create(d, 32), ASE_INVALID_MODE, "only the instance's block");
    assert_eq!(create(d, 64), ASE_OK);
    // Attached only while the host's callbacks run: from start to stop. The
    // device expects an answer to every block while attached, and counts
    // the blocks without one as late.
    assert_eq!(
        device.header().host_attached.load(Ordering::Acquire),
        0,
        "buffers alone do not answer blocks"
    );
    // SAFETY: as a host.
    assert_eq!(unsafe { (vt(d).start)(d) }, ASE_OK);
    assert_eq!(device.header().host_attached.load(Ordering::Acquire), 1);
    // The device publishes block 3 in half 1: a ramp on its input.
    let input = device.buffer(1, true, 0).unwrap();
    for k in 0..64 {
        // SAFETY: 64 samples.
        unsafe { input.add(k).write((k as i32 + 1) << 8) };
    }
    device.header().seq.store(3, Ordering::Release);
    device.signal_block();
    assert!(device.wait_ready(1000), "the driver returned the outputs");
    assert_eq!(device.header().ack.load(Ordering::Acquire), 3);
    assert_eq!(LAST_HALF.load(Ordering::Acquire), 1);
    let output = device.buffer(1, false, 0).unwrap();
    for k in 0..64 {
        // SAFETY: 64 samples.
        assert_eq!(unsafe { output.add(k).read() }, (k as i32 + 1) << 8);
    }
    // SAFETY: as a host ends.
    unsafe {
        assert_eq!((vt(d).stop)(d), ASE_OK);
        assert_eq!(
            device.header().host_attached.load(Ordering::Acquire),
            0,
            "a stopped host answers no block"
        );
        assert_eq!((vt(d).disposeBuffers)(d), ASE_OK);
        assert_eq!(device.header().host_attached.load(Ordering::Acquire), 0);
        (vt(d).Release)(d);
    }
}

#[test]
fn no_device_free_runs_silence() {
    let d = driver(6);
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        let (mut i, mut o) = (0, 0);
        assert_eq!((vt(d).getChannels)(d, &mut i, &mut o), ASE_OK);
        assert_eq!((i, o), (8, 8));
    }
    let mut infos: Vec<ASIOBufferInfo> = (0..2)
        .map(|k| ASIOBufferInfo {
            isInput: k,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(count_only),
        sampleRateDidChange: None,
        asioMessage: Some(message),
        bufferSwitchTimeInfo: None,
    };
    // SAFETY: as a host.
    unsafe {
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 32, &cb),
            ASE_OK
        );
        assert!(!infos[0].buffers[0].is_null());
        assert_eq!((vt(d).start)(d), ASE_OK);
    }
    let t = Instant::now();
    while FREE.load(Ordering::Acquire) < 50 && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        FREE.load(Ordering::Acquire) >= 50,
        "the DAW is called on its own clock"
    );
    // SAFETY: as a host ends.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}

static FREE: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_only(_half: i32, _direct: ASIOBool) {
    FREE.fetch_add(1, Ordering::AcqRel);
}

// A host that counts the driver's reset requests.
static RESETS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_resets(sel: i32, _val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    if sel == K_ASIO_RESET_REQUEST {
        RESETS.fetch_add(1, Ordering::AcqRel);
    }
    0
}

unsafe extern "C" fn ignore(_half: i32, _direct: ASIOBool) {}

#[test]
fn a_reshaped_device_gets_one_reset_request_within_the_budget() {
    let shape = Shape::new(1, 1, 32).unwrap();
    let device = Shared::create(9, shape, 96_000, (56, 40)).unwrap();
    let d = driver(9);
    let mut infos: Vec<ASIOBufferInfo> = (0..2)
        .map(|k| ASIOBufferInfo {
            isInput: k,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(ignore),
        sampleRateDidChange: None,
        asioMessage: Some(count_resets),
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
    let wait_for = |n: u32, limit: Duration| {
        let t = Instant::now();
        while RESETS.load(Ordering::Acquire) < n && t.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // The device came back with another block (its tray changed it).
    device.header().block.store(64, Ordering::Release);
    wait_for(1, Duration::from_secs(3));
    assert_eq!(RESETS.load(Ordering::Acquire), 1, "one reset request");
    // And changed again at once: the budget holds the next request back.
    device.header().block.store(128, Ordering::Release);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        RESETS.load(Ordering::Acquire),
        1,
        "no second request within 5 min"
    );
    // SAFETY: as a host ends.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}

// A host whose callback never returns (until the test lets it go).
static HUNG_IN: AtomicU32 = AtomicU32::new(0);
static LET_GO: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn hang(_half: i32, _direct: ASIOBool) {
    HUNG_IN.store(1, Ordering::Release);
    let t = Instant::now();
    while LET_GO.load(Ordering::Acquire) == 0 && t.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn stop_returns_while_the_host_s_callback_hangs() {
    let d = driver(7);
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
    let t = Instant::now();
    // SAFETY: as a host.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
    }
    let took = t.elapsed();
    LET_GO.store(1, Ordering::Release);
    // SAFETY: as a host ends.
    unsafe { (vt(d).Release)(d) };
    assert!(
        took < Duration::from_secs(3),
        "stop and disposeBuffers waited {took:?} for a hung callback"
    );
}

#[test]
fn output_buffers_start_silent() {
    let shape = Shape::new(1, 2, 32).unwrap();
    let device = Shared::create(8, shape, 96_000, (56, 40)).unwrap();
    // What a previous session left in the outputs.
    for half in 0..2 {
        for ch in 0..2 {
            let p = device.buffer(half, false, ch).unwrap();
            for k in 0..32 {
                // SAFETY: 32 samples.
                unsafe { p.add(k).write(0x7f00) };
            }
        }
    }
    let d = driver(8);
    let mut infos: Vec<ASIOBufferInfo> = [(1, 0), (0, 0), (0, 1)]
        .iter()
        .map(|&(input, ch)| ASIOBufferInfo {
            isInput: input,
            channelNum: ch,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(count_only),
        sampleRateDidChange: None,
        asioMessage: Some(message),
        bufferSwitchTimeInfo: None,
    };
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 3, 32, &cb),
            ASE_OK
        );
    }
    for info in &infos[1..] {
        for half in 0..2 {
            let p = info.buffers[half] as *const i32;
            for k in 0..32 {
                // SAFETY: an output buffer of 32 samples the driver handed out.
                assert_eq!(
                    unsafe { p.add(k).read() },
                    0,
                    "output {} half {half}",
                    info.channelNum
                );
            }
        }
    }
    // SAFETY: as a host ends.
    unsafe {
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}

#[test]
fn an_unknown_clsid_is_not_available() {
    let mut other = clsid(1).unwrap();
    other.data1 ^= 0xff;
    let mut f: *mut c_void = core::ptr::null_mut();
    // SAFETY: valid pointers.
    let hr = unsafe { dasio_driver::DllGetClassObject(&other, &IID_ICLASSFACTORY, &mut f) };
    assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
    assert!(f.is_null());
}

#[test]
fn blocks_the_driver_skipped_are_counted_in_the_header() {
    let shape = Shape::new(1, 1, 32).unwrap();
    let device = Shared::create(10, shape, 96_000, (56, 40)).unwrap();
    let d = driver(10);
    let mut infos: Vec<ASIOBufferInfo> = (0..2)
        .map(|k| ASIOBufferInfo {
            isInput: k,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(ignore),
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
    let h = device.header();
    h.seq.store(1, Ordering::Release);
    device.signal_block();
    assert!(device.wait_ready(1000), "block 1 handed over");
    assert_eq!(h.ack.load(Ordering::Acquire), 1);
    assert_eq!(h.missed.load(Ordering::Acquire), 0, "nothing skipped yet");
    // Blocks 2 and 3 were published while the driver looked away.
    h.seq.store(4, Ordering::Release);
    device.signal_block();
    assert!(device.wait_ready(1000), "block 4 handed over");
    assert_eq!(h.ack.load(Ordering::Acquire), 4);
    assert_eq!(h.missed.load(Ordering::Acquire), 2);
    // SAFETY: as a host ends.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}
