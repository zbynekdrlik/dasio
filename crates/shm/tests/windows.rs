#![cfg(windows)]

use std::sync::atomic::Ordering;

use shm::{MAGIC, Shape, Shared};

#[test]
fn create_then_open_shares_the_buffers_and_the_header() {
    let shape = Shape::new(4, 2, 64).unwrap();
    let device = Shared::create(91, shape, 96_000, (88, 40)).unwrap();
    let driver = Shared::open(91).unwrap();
    assert_eq!(driver.shape(), shape);
    assert_eq!(driver.header().magic.load(Ordering::Acquire), MAGIC);
    assert_eq!(driver.header().latency_in.load(Ordering::Relaxed), 88);
    assert_eq!(driver.header().latency_out.load(Ordering::Relaxed), 40);
    let input = device.buffer(1, true, 3).unwrap();
    // SAFETY: a buffer of 64 samples inside the mapping.
    unsafe { input.add(63).write(0x7f00_0000) };
    let seen = driver.buffer(1, true, 3).unwrap();
    // SAFETY: the same buffer through the other view.
    assert_eq!(unsafe { seen.add(63).read() }, 0x7f00_0000);
    device.header().seq.store(7, Ordering::Release);
    assert_eq!(driver.header().seq.load(Ordering::Acquire), 7);
}

#[test]
fn the_events_cross_between_the_sides() {
    let shape = Shape::new(1, 1, 32).unwrap();
    let device = Shared::create(92, shape, 96_000, (56, 40)).unwrap();
    let driver = Shared::open(92).unwrap();
    assert!(!driver.wait_block(10), "nothing published yet");
    device.signal_block();
    assert!(driver.wait_block(1000));
    assert!(!driver.wait_block(10), "auto-reset: one signal, one wake");
    driver.signal_ready();
    assert!(device.wait_ready(1000));
}

#[test]
fn opening_an_instance_no_device_made_is_not_found() {
    let e = Shared::open(93).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
}

// A DAW may run as another user than the device (the FOH PC's master Ableton
// does) and the device runs elevated: without an explicit DACL only the
// device's own user reaches the objects, and the driver silently free-runs.
#[test]
fn any_signed_in_user_reaches_the_objects() {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::System::Memory::OpenFileMappingW;
    use windows_sys::Win32::System::Threading::OpenEventW;

    const READ_CONTROL: u32 = 0x0002_0000;
    let instance = 77;
    let _device = Shared::create(instance, Shape::new(2, 2, 64).unwrap(), 96000, (0, 0)).unwrap();
    for what in ["shm", "block", "ready"] {
        let n: Vec<u16> = format!("Global\\dasio-{instance}-{what}\0")
            .encode_utf16()
            .collect();
        // SAFETY: a valid NUL-terminated name.
        let h = unsafe {
            if what == "shm" {
                OpenFileMappingW(READ_CONTROL, 0, n.as_ptr())
            } else {
                OpenEventW(READ_CONTROL, 0, n.as_ptr())
            }
        };
        assert!(!h.is_null(), "{what}");
        let mut sd = std::ptr::null_mut();
        let mut text = std::ptr::null_mut();
        // SAFETY: a valid handle; the out pointers live; both results freed.
        let sddl = unsafe {
            assert_eq!(
                GetSecurityInfo(
                    h,
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut sd,
                ),
                0
            );
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    sd,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut(),
                ),
                0
            );
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            LocalFree(sd);
            CloseHandle(h);
            s
        };
        assert!(sddl.contains(";;;AU)"), "{what}: {sddl}");
    }
}

#[test]
fn a_watcher_peeks_at_the_header_read_only() {
    let shape = Shape::new(3, 2, 128).unwrap();
    assert_eq!(
        shm::peek(93).unwrap_err().kind(),
        std::io::ErrorKind::NotFound,
        "no device yet"
    );
    let device = Shared::create(93, shape, 96_000, (224, 352)).unwrap();
    device.header().late.store(4, Ordering::Relaxed);
    device.header().host_attached.store(1, Ordering::Release);
    let s = shm::peek(93).unwrap();
    assert_eq!((s.rx, s.tx, s.block, s.rate), (3, 2, 128, 96_000));
    assert_eq!((s.latency_in, s.latency_out), (224, 352));
    assert_eq!(s.device_pid, std::process::id());
    assert_eq!(s.late, 4);
    assert!(s.attached);
    drop(device);
    assert!(shm::peek(93).is_err(), "gone with the device");
}
