//! The driver through its COM entry, as a DAW drives it, with a stand-in
//! device on the instance's shared memory (plan Task 6).
#![cfg(windows)]

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use dasio_driver::ident::clsid;
use shm::{HostUse, Shape, Shared};

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
    // dasio#28 E4: the device sees each read of the latencies.
    assert_eq!(device.header().latency_reads.load(Ordering::Acquire), 1);
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
    assert_eq!(
        device.host_pid(),
        std::process::id(),
        "the device knows the DAW's process while it has the buffers"
    );
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
    // A host with plain bufferSwitch and no outputReady: the driver
    // returned the block itself, and the device sees neither use.
    assert!(!device.header().uses(HostUse::OutputReady));
    assert!(!device.header().uses(HostUse::TimeInfo));
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
        assert_eq!(device.host_pid(), 0, "nor is it the DAW any more");
        assert_eq!((vt(d).disposeBuffers)(d), ASE_OK);
        assert_eq!(device.header().host_attached.load(Ordering::Acquire), 0);
        (vt(d).Release)(d);
    }
}

// dasio#28 A91: a host started before its device free-runs on the
// driver's clock; once the device appears the driver asks it for one reset,
// and the host's next driver attaches. E4: the device then sees how the
// host drives it.
#[test]
fn a_host_started_before_the_device_free_runs_then_attaches() {
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
        asioMessage: Some(count_early_resets),
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
    assert_eq!(EARLY_RESETS.load(Ordering::Acquire), 0);

    // The device starts now.
    let device = Shared::create(6, Shape::new(1, 1, 64).unwrap(), 96_000, (88, 40)).unwrap();
    let t = Instant::now();
    while EARLY_RESETS.load(Ordering::Acquire) == 0 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        EARLY_RESETS.load(Ordering::Acquire),
        1,
        "one reset request once the device appears"
    );
    // As a DAW answers a reset: this driver goes, a new one opens the device.
    // SAFETY: as a host.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
    let d = driver(6);
    TIME_HOST.store(d, Ordering::Release);
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
    let cb = ASIOCallbacks {
        bufferSwitch: None,
        sampleRateDidChange: None,
        asioMessage: Some(takes_time_info),
        bufferSwitchTimeInfo: Some(switch_then_ready),
    };
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        let (mut i, mut o) = (0, 0);
        assert_eq!((vt(d).getLatencies)(d, &mut i, &mut o), ASE_OK);
        assert_eq!((i, o), (88, 40));
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 64, &cb),
            ASE_OK
        );
        assert_eq!((vt(d).start)(d), ASE_OK);
    }
    assert_eq!(device.header().host_attached.load(Ordering::Acquire), 1);
    device.header().seq.store(1, Ordering::Release);
    device.signal_block();
    assert!(
        device.wait_ready(1000),
        "the attached host returned block 1"
    );
    assert_eq!(device.header().ack.load(Ordering::Acquire), 1);
    assert!(TIME_SWITCHES.load(Ordering::Acquire) >= 1);
    // E4: what the device's counts line shows of this host.
    let h = device.header();
    assert!(h.uses(HostUse::OutputReady), "the host called outputReady");
    assert!(
        h.uses(HostUse::TimeInfo),
        "the host takes bufferSwitchTimeInfo"
    );
    assert_eq!(h.latency_reads.load(Ordering::Acquire), 1);
    // SAFETY: as a host ends.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}

// The early host's reset requests (instance 6).
static EARLY_RESETS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_early_resets(sel: i32, _v: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    if sel == K_ASIO_RESET_REQUEST {
        EARLY_RESETS.fetch_add(1, Ordering::AcqRel);
    }
    0
}

// A host that takes bufferSwitchTimeInfo and calls outputReady from it
// (instance 6's second driver).
static TIME_HOST: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static TIME_SWITCHES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn takes_time_info(sel: i32, val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    match (sel, val) {
        (K_ASIO_SELECTOR_SUPPORTED, K_ASIO_SUPPORTS_TIME_INFO) | (K_ASIO_SUPPORTS_TIME_INFO, _) => {
            1
        }
        _ => 0,
    }
}

unsafe extern "C" fn switch_then_ready(
    t: *mut ASIOTime,
    _half: i32,
    _direct: ASIOBool,
) -> *mut ASIOTime {
    TIME_SWITCHES.fetch_add(1, Ordering::AcqRel);
    let d = TIME_HOST.load(Ordering::Acquire);
    // SAFETY: the live driver this host opened.
    unsafe { (vt(d).outputReady)(d) };
    t
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
    assert_eq!(device.header().messages(), (1, 0), "counted for the device");
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

// A host that counts reset requests for the restart test (its own counter:
// the reshape test counts RESETS).
static RESTART_RESETS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_restart_resets(
    sel: i32,
    _val: i32,
    _m: *mut c_void,
    _o: *mut f64,
) -> i32 {
    if sel == K_ASIO_RESET_REQUEST {
        RESTART_RESETS.fetch_add(1, Ordering::AcqRel);
    }
    0
}

// The device restarts with another block while the DAW runs on the old
// one: the driver asks the DAW for one reset, and the driver the DAW then
// makes opens the new layout (PR #4 review).
#[test]
fn a_device_restart_with_another_block_asks_one_reset_and_the_new_driver_sees_it() {
    let device = Shared::create(11, Shape::new(1, 1, 32).unwrap(), 96_000, (56, 40)).unwrap();
    let d = driver(11);
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
        asioMessage: Some(count_restart_resets),
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
    drop(device);
    let device = Shared::create(11, Shape::new(1, 1, 64).unwrap(), 96_000, (88, 40)).unwrap();
    let t = Instant::now();
    while RESTART_RESETS.load(Ordering::Acquire) < 1 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        RESTART_RESETS.load(Ordering::Acquire),
        1,
        "one reset request"
    );
    // The host resets: the old driver goes, a new one opens the device.
    // SAFETY: as a host.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
    let d = driver(11);
    let (mut min, mut max, mut pref, mut gran) = (0, 0, 0, 0);
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        assert_eq!(
            (vt(d).getBufferSize)(d, &mut min, &mut max, &mut pref, &mut gran),
            ASE_OK
        );
        (vt(d).Release)(d);
    }
    assert_eq!(
        pref, 64,
        "the new driver opens the restarted device's block"
    );
    drop(device);
}

static REINIT_RESETS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_reinit_resets(
    sel: i32,
    _val: i32,
    _m: *mut c_void,
    _o: *mut f64,
) -> i32 {
    if sel == K_ASIO_RESET_REQUEST {
        REINIT_RESETS.fetch_add(1, Ordering::AcqRel);
    }
    0
}

// A host that answers the reset by re-initing the same driver object (not
// by making a new one) must reach the restarted device's new layout too
// (PR #4 review).
#[test]
fn a_host_that_re_inits_the_same_driver_after_a_reset_sees_the_new_block() {
    let device = Shared::create(12, Shape::new(1, 1, 32).unwrap(), 96_000, (56, 40)).unwrap();
    let d = driver(12);
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
        asioMessage: Some(count_reinit_resets),
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
    drop(device);
    let device = Shared::create(12, Shape::new(1, 1, 64).unwrap(), 96_000, (88, 40)).unwrap();
    let t = Instant::now();
    while REINIT_RESETS.load(Ordering::Acquire) < 1 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        REINIT_RESETS.load(Ordering::Acquire),
        1,
        "one reset request"
    );
    let (mut min, mut max, mut pref, mut gran) = (0, 0, 0, 0);
    // SAFETY: as a host: stop, dispose, init the same object again.
    unsafe {
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        assert_eq!(
            (vt(d).getBufferSize)(d, &mut min, &mut max, &mut pref, &mut gran),
            ASE_OK
        );
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 64, &cb),
            ASE_OK
        );
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
    assert_eq!(
        pref, 64,
        "the re-inited driver opens the restarted device's block"
    );
    drop(device);
}

// A host that stops the driver from inside its kAsioLatenciesChanged
// handler, on the driver's own thread (dasio#28 E16).
static INNER: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static INNER_STOP_MS: AtomicU32 = AtomicU32::new(u32::MAX);
static INNER_STOPPED: AtomicBool = AtomicBool::new(false);
static SWITCHES_AFTER_STOP: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_after_stop(_half: i32, _direct: ASIOBool) {
    if INNER_STOPPED.load(Ordering::Acquire) {
        SWITCHES_AFTER_STOP.fetch_add(1, Ordering::AcqRel);
    }
}

unsafe extern "C" fn stop_inside(sel: i32, val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    if sel == K_ASIO_SELECTOR_SUPPORTED {
        return i32::from(val == K_ASIO_LATENCIES_CHANGED);
    }
    if sel == K_ASIO_LATENCIES_CHANGED {
        let d = INNER.load(Ordering::Acquire);
        let t = Instant::now();
        // SAFETY: the live driver, stopped by its host right here.
        unsafe { (vt(d).stop)(d) };
        INNER_STOP_MS.store(t.elapsed().as_millis() as u32, Ordering::Release);
        INNER_STOPPED.store(true, Ordering::Release);
        return 1;
    }
    0
}

#[test]
fn a_host_that_stops_the_driver_inside_a_message_gets_no_more_blocks() {
    let shape = Shape::new(1, 1, 32).unwrap();
    let device = Shared::create(16, shape, 96_000, (56, 40)).unwrap();
    let d = driver(16);
    INNER.store(d, Ordering::Release);
    let mut infos: Vec<ASIOBufferInfo> = (0..2)
        .map(|k| ASIOBufferInfo {
            isInput: k,
            channelNum: 0,
            buffers: [core::ptr::null_mut(); 2],
        })
        .collect();
    let cb = ASIOCallbacks {
        bufferSwitch: Some(count_after_stop),
        sampleRateDidChange: None,
        asioMessage: Some(stop_inside),
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
    // The device publishes a block every ms; after 200 ms its latencies
    // change, and it goes on for 300 ms after the host's stop.
    let h = device.header();
    let t = Instant::now();
    let mut stopped_at: Option<Instant> = None;
    let mut seq = 0u64;
    while t.elapsed() < Duration::from_secs(5) {
        seq += 1;
        h.seq.store(seq, Ordering::Release);
        device.signal_block();
        if t.elapsed() > Duration::from_millis(200) {
            h.latency_in.store(80, Ordering::Release);
        }
        if stopped_at.is_none() && INNER_STOPPED.load(Ordering::Acquire) {
            stopped_at = Some(Instant::now());
        }
        if stopped_at.is_some_and(|s| s.elapsed() > Duration::from_millis(300)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(stopped_at.is_some(), "the latency change reached the host");
    let ms = INNER_STOP_MS.load(Ordering::Acquire);
    assert!(ms < 200, "stop inside the host's callback took {ms} ms");
    assert_eq!(
        SWITCHES_AFTER_STOP.load(Ordering::Acquire),
        0,
        "no block handed over after the host stopped the driver"
    );
    assert_eq!(device.header().messages(), (0, 1), "counted for the device");
    // SAFETY: as a host: it starts the driver again and ends.
    unsafe {
        assert_eq!((vt(d).start)(d), ASE_OK);
        (vt(d).stop)(d);
        (vt(d).disposeBuffers)(d);
        (vt(d).Release)(d);
    }
}

// dasio#28 E17: the device dies while the DAW plays, then comes back. The
// DAW keeps being called on the driver's clock with silent inputs, its
// getErrorMessage names the stopped device, and once the device beats and
// publishes again its blocks reach the DAW again (instance 4).
static DEATH_SWITCHES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_death(_half: i32, _direct: ASIOBool) {
    DEATH_SWITCHES.fetch_add(1, Ordering::AcqRel);
}

/// A stand-in device on instance 4: it beats and publishes a block of a
/// non-zero input every ms until `run` is cleared.
fn play(device: &Shared, run: &AtomicBool, from_seq: u64) -> u64 {
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

/// `getErrorMessage`'s text.
fn error_text(d: *mut c_void) -> String {
    let mut text = [0 as core::ffi::c_char; 128];
    // SAFETY: as a host: a buffer of at least 124 characters.
    unsafe { (vt(d).getErrorMessage)(d, text.as_mut_ptr()) };
    let bytes: Vec<u8> = text
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn a_device_that_dies_while_the_daw_plays_leaves_it_running_and_comes_back() {
    let device = Shared::create(4, Shape::new(1, 1, 64).unwrap(), 96_000, (88, 40)).unwrap();
    device.header().heartbeat_ms.store(1, Ordering::Release);
    let d = driver(4);
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
    let cb = ASIOCallbacks {
        bufferSwitch: Some(count_death),
        sampleRateDidChange: None,
        asioMessage: None,
        bufferSwitchTimeInfo: None,
    };
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 64, &cb),
            ASE_OK
        );
        assert_eq!((vt(d).start)(d), ASE_OK);
    }
    let run = AtomicBool::new(true);
    // Alive: the DAW answers the device's blocks.
    let last = std::thread::scope(|s| {
        let h = s.spawn(|| play(&device, &run, 0));
        std::thread::sleep(Duration::from_millis(300));
        run.store(false, Ordering::Release);
        h.join().unwrap()
    });
    assert!(
        device.header().ack.load(Ordering::Acquire) > 0,
        "the DAW answered the device's blocks"
    );
    assert_eq!(error_text(d), "", "a live device has nothing to report");

    // Dead: no block, no heartbeat. The DAW is still called, with silence
    // in: once per QUIET (50 ms) without a block, so about 32 times in
    // 1.6 s, not once per block period (dasio#28 E17 note).
    let before = DEATH_SWITCHES.load(Ordering::Acquire);
    std::thread::sleep(Duration::from_millis(1600));
    let during = DEATH_SWITCHES.load(Ordering::Acquire) - before;
    assert!(
        during >= 20,
        "the DAW is still called while the device is dead: {during} switches in 1.6 s"
    );
    for half in 0..2 {
        let input = device.buffer(half, true, 0).unwrap();
        // SAFETY: 64 samples of the input buffer; the device is not writing.
        let silent = (0..64).all(|k| unsafe { input.add(k).read() } == 0);
        assert!(
            silent,
            "half {half}'s input is silence while the device is dead"
        );
    }
    assert_eq!(error_text(d), "the device stopped (no heartbeat)");

    // Back: it beats and publishes again, and its blocks reach the DAW.
    let acked = device.header().ack.load(Ordering::Acquire);
    run.store(true, Ordering::Release);
    std::thread::scope(|s| {
        let h = s.spawn(|| play(&device, &run, last));
        std::thread::sleep(Duration::from_millis(1500));
        run.store(false, Ordering::Release);
        h.join().unwrap();
    });
    assert!(
        device.header().ack.load(Ordering::Acquire) > acked,
        "the DAW answers the device again once it is back"
    );
    assert_eq!(
        error_text(d),
        "",
        "a device back alive has nothing to report"
    );
    // SAFETY: as a host ends.
    unsafe {
        assert_eq!((vt(d).stop)(d), ASE_OK);
        assert_eq!((vt(d).disposeBuffers)(d), ASE_OK);
        (vt(d).Release)(d);
    }
}
