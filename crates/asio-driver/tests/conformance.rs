//! dasio#28 E17: the driver through its COM entry, answering the call
//! sequence of an ASIOTest-style host as the ASIO SDK says.
#![cfg(windows)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dasio_driver::abi::*;
use shm::{Shape, Shared};

mod common;
use common::{driver, play, vt};

unsafe extern "C" fn message(_sel: i32, _val: i32, _m: *mut c_void, _o: *mut f64) -> i32 {
    0
}

// dasio#28 E17: an ASIOTest-style conformance run. The driver walks the
// call sequence Steinberg's ASIOTest makes, and every answer is the one the
// ASIO SDK specifies (instance 3: the host tests use 4 to 12 and 16, the
// e2e test 13 and 15, the service test 14).
static CONF_SWITCHES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_conformance(_half: i32, _direct: ASIOBool) {
    CONF_SWITCHES.fetch_add(1, Ordering::AcqRel);
}

/// Clears `run` when dropped: ends [`play`] on any way out of a scope.
struct EndOnDrop<'a>(&'a AtomicBool);

impl Drop for EndOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// `getChannelInfo` for one channel: its answer and what it filled in.
fn channel_info(d: *mut c_void, input: bool, channel: i32) -> (ASIOError, ASIOChannelInfo) {
    let mut info = ASIOChannelInfo {
        channel,
        isInput: ASIOBool::from(input),
        isActive: -1,
        channelGroup: -1,
        type_: -1,
        name: [0; 32],
    };
    // SAFETY: as a host: `channel` and `isInput` set.
    let e = unsafe { (vt(d).getChannelInfo)(d, &mut info) };
    (e, info)
}

/// `getSamplePosition`'s answer and the position it gave.
fn sample_position(d: *mut c_void) -> (ASIOError, u64) {
    let (mut pos, mut stamp) = (Asio64::default(), Asio64::default());
    // SAFETY: as a host: two valid out-pointers.
    let e = unsafe { (vt(d).getSamplePosition)(d, &mut pos, &mut stamp) };
    (e, pos.get())
}

/// The channels whose `isActive` is true, as (isInput, channel).
fn active_channels(d: *mut c_void, ins: i32, outs: i32) -> Vec<(bool, i32)> {
    let mut active = Vec::new();
    for (input, count) in [(true, ins), (false, outs)] {
        for ch in 0..count {
            let (e, info) = channel_info(d, input, ch);
            assert_eq!(e, ASE_OK, "getChannelInfo({input}, {ch})");
            assert!(
                info.isActive == ASIO_TRUE || info.isActive == ASIO_FALSE,
                "isActive is a plain ASIOBool: {}",
                info.isActive
            );
            if info.isActive == ASIO_TRUE {
                active.push((input, ch));
            }
        }
    }
    active
}

#[test]
fn the_driver_answers_an_asio_test_host_s_call_sequence_as_the_sdk_says() {
    let device = Shared::create(3, Shape::new(2, 2, 64).unwrap(), 96_000, (88, 40)).unwrap();
    device.header().heartbeat_ms.store(1, Ordering::Release);
    let d = driver(3);
    let (ins, outs) = (2, 2);
    // SAFETY: the driver's methods as a host calls them.
    unsafe {
        // Loading: init, the driver's name and version.
        assert_eq!((vt(d).init)(d, core::ptr::null_mut()), ASIO_TRUE);
        let mut name = [0 as core::ffi::c_char; 32];
        (vt(d).getDriverName)(d, name.as_mut_ptr());
        assert_ne!(name[0], 0, "getDriverName gives a name");
        assert!(name.contains(&0), "the name ends inside its 32 bytes");
        assert!((vt(d).getDriverVersion)(d) > 0);

        // The device's shape: channels, the one buffer size, the latencies.
        let (mut i, mut o) = (0, 0);
        assert_eq!((vt(d).getChannels)(d, &mut i, &mut o), ASE_OK);
        assert_eq!((i, o), (ins, outs));
        let (mut min, mut max, mut pref, mut gran) = (0, 0, 0, -1);
        assert_eq!(
            (vt(d).getBufferSize)(d, &mut min, &mut max, &mut pref, &mut gran),
            ASE_OK
        );
        assert_eq!((min, max, pref, gran), (64, 64, 64, 0));
        let (mut lat_in, mut lat_out) = (0, 0);
        assert_eq!((vt(d).getLatencies)(d, &mut lat_in, &mut lat_out), ASE_OK);
        assert_eq!((lat_in, lat_out), (88, 40), "the device's latencies");

        // Sample rates: 96 kHz only; any other has no clock.
        assert_eq!((vt(d).canSampleRate)(d, 96_000.0), ASE_OK);
        assert_eq!((vt(d).canSampleRate)(d, 44_100.0), ASE_NO_CLOCK);
        let mut rate = 0.0;
        assert_eq!((vt(d).getSampleRate)(d, &mut rate), ASE_OK);
        assert_eq!(rate, 96_000.0);
        assert_eq!((vt(d).setSampleRate)(d, 96_000.0), ASE_OK);
        assert_eq!((vt(d).setSampleRate)(d, 48_000.0), ASE_NO_CLOCK);

        // Clock sources: one, the current one; only index 0 is settable.
        let mut clocks: [ASIOClockSource; 4] = core::array::from_fn(|_| ASIOClockSource {
            index: -1,
            associatedChannel: 0,
            associatedGroup: 0,
            isCurrentSource: ASIO_FALSE,
            name: [0; 32],
        });
        let mut n = clocks.len() as i32;
        assert_eq!(
            (vt(d).getClockSources)(d, clocks.as_mut_ptr(), &mut n),
            ASE_OK
        );
        assert_eq!(n, 1, "one clock source");
        assert_eq!(clocks[0].index, 0);
        assert_eq!(clocks[0].isCurrentSource, ASIO_TRUE);
        assert_ne!(clocks[0].name[0], 0, "the clock source has a name");
        assert_eq!((vt(d).setClockSource)(d, 0), ASE_OK);
        assert_eq!((vt(d).setClockSource)(d, 1), ASE_INVALID_PARAMETER);
    }

    // Every channel: 32-bit LSB samples, group 0, inactive before
    // createBuffers. One past the count is an invalid parameter.
    for (input, count) in [(true, ins), (false, outs)] {
        for ch in 0..count {
            let (e, info) = channel_info(d, input, ch);
            assert_eq!(e, ASE_OK, "getChannelInfo({input}, {ch})");
            assert_eq!(info.type_, ASIOST_INT32_LSB);
            assert_eq!(info.channelGroup, 0);
            assert_ne!(info.name[0], 0, "channel ({input}, {ch}) has a name");
        }
        assert_eq!(
            channel_info(d, input, count).0,
            ASE_INVALID_PARAMETER,
            "channel {count} of {count} ({input})"
        );
        assert_eq!(channel_info(d, input, -1).0, ASE_INVALID_PARAMETER);
    }
    assert_eq!(
        active_channels(d, ins, outs),
        Vec::<(bool, i32)>::new(),
        "no channel is active before createBuffers"
    );

    // Not started: the position does not advance.
    assert_eq!(sample_position(d).0, ASE_SP_NOT_ADVANCING);

    let cb = ASIOCallbacks {
        bufferSwitch: Some(count_conformance),
        sampleRateDidChange: None,
        asioMessage: Some(message),
        bufferSwitchTimeInfo: None,
    };
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
    // SAFETY: as a host.
    unsafe {
        assert_eq!((vt(d).start)(d), ASE_INVALID_MODE, "start needs buffers");
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 32, &cb),
            ASE_INVALID_MODE,
            "a buffer size outside getBufferSize's range"
        );
        assert_eq!(
            (vt(d).createBuffers)(d, infos.as_mut_ptr(), 2, 64, &cb),
            ASE_OK
        );
    }
    for info in &infos {
        // A copy: `buffers` is a field of a packed struct.
        let [first, second] = info.buffers;
        assert!(
            !first.is_null() && !second.is_null(),
            "createBuffers gives both halves"
        );
        assert_ne!(first, second, "two distinct halves");
    }
    assert_eq!(
        active_channels(d, ins, outs),
        vec![(true, 0), (false, 0)],
        "exactly the channels createBuffers made are active"
    );
    assert_eq!(
        sample_position(d).0,
        ASE_SP_NOT_ADVANCING,
        "buffers alone do not advance the position"
    );

    // Started: blocks reach the host and the position advances.
    // SAFETY: as a host.
    assert_eq!(unsafe { (vt(d).start)(d) }, ASE_OK);
    let run = AtomicBool::new(true);
    std::thread::scope(|s| {
        let h = s.spawn(|| play(&device, &run, 0));
        // A failed assert below still ends the stand-in device, so the
        // scope's join returns and the test fails instead of hanging.
        let _end = EndOnDrop(&run);
        let deadline = Instant::now() + Duration::from_secs(2);
        while CONF_SWITCHES.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            CONF_SWITCHES.load(Ordering::Acquire) >= 2,
            "bufferSwitch arrives after start"
        );
        let (e, first) = sample_position(d);
        assert_eq!(e, ASE_OK, "a started driver's position advances");
        std::thread::sleep(Duration::from_millis(100));
        let (e, later) = sample_position(d);
        assert_eq!(e, ASE_OK);
        assert!(later > first, "the position advances: {first} then {later}");
        // SAFETY: as a host.
        unsafe {
            assert_eq!((vt(d).outputReady)(d), ASE_OK);
            assert_eq!(
                (vt(d).future)(d, K_ASIO_CAN_TIME_INFO, core::ptr::null_mut()),
                ASE_SUCCESS
            );
            assert_eq!(
                (vt(d).future)(d, 0x7fff_0000, core::ptr::null_mut()),
                ASE_NOT_PRESENT,
                "an unknown selector"
            );
        }
        run.store(false, Ordering::Release);
        h.join().unwrap();
    });

    // Stopped: the position stops; disposed: every channel inactive again.
    // SAFETY: as a host ends.
    unsafe {
        assert_eq!((vt(d).stop)(d), ASE_OK);
        assert_eq!(sample_position(d).0, ASE_SP_NOT_ADVANCING);
        assert_eq!(
            active_channels(d, ins, outs),
            vec![(true, 0), (false, 0)],
            "a stopped driver keeps its buffers"
        );
        assert_eq!((vt(d).disposeBuffers)(d), ASE_OK);
        assert_eq!(
            active_channels(d, ins, outs),
            Vec::<(bool, i32)>::new(),
            "no channel is active after disposeBuffers"
        );
        assert_eq!(sample_position(d).0, ASE_SP_NOT_ADVANCING);
        (vt(d).Release)(d);
    }
}
