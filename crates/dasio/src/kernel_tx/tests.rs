use super::*;

fn frame(len: usize, tag: u8) -> Vec<u8> {
    let mut f = vec![0u8; len];
    f[12] = 0x08;
    f[13] = 0x00;
    f[14] = 0x45;
    f[len - 1] = tag;
    f
}

#[test]
fn the_ioctl_code_is_filteruser_h_s() {
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 21, METHOD_BUFFERED, FILE_WRITE_ACCESS)
    assert_eq!(IOCTL_DASIO_TX_SEND, 0x0017_8054);
    // The sample's own codes use FILE_ANY_ACCESS: function 14 is 0x170038.
    assert_eq!(ctl_code(0x17, 14, 0, 0), 0x0017_0038);
    // Every field in its place: FILE_DEVICE_UNKNOWN, 0x801, METHOD_NEITHER,
    // FILE_READ_ACCESS (winioctl.h's arithmetic).
    assert_eq!(ctl_code(0x22, 0x801, 3, 1), 0x0022_6007);
}

#[test]
fn a_batch_packs_header_then_length_and_bytes_per_frame() {
    let a = frame(MIN_FRAME, 1);
    let b = frame(MAX_FRAME, 2);
    let mut buf = Vec::with_capacity(4096);
    pack(&[&a, &b], 7, &mut buf).unwrap();
    assert_eq!(&buf[..8], &[7, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&buf[8..10], &(MIN_FRAME as u16).to_le_bytes());
    assert_eq!(&buf[10..10 + MIN_FRAME], &a[..]);
    let at = 10 + MIN_FRAME;
    assert_eq!(&buf[at..at + 2], &(MAX_FRAME as u16).to_le_bytes());
    assert_eq!(&buf[at + 2..], &b[..]);
    // A second batch replaces the first.
    pack(&[&a], 9, &mut buf).unwrap();
    assert_eq!(buf.len(), HEADER + 2 + MIN_FRAME);
    assert_eq!(buf[0], 9);
}

#[test]
fn packing_never_grows_the_buffer() {
    let a = frame(1000, 0);
    let mut buf = Vec::with_capacity(HEADER + 2 + 1000);
    pack(&[&a], 1, &mut buf).unwrap();
    let cap = buf.capacity();
    assert_eq!(pack(&[&a, &a], 1, &mut buf), Err(PackError::Full));
    assert_eq!(buf.capacity(), cap);
}

#[test]
fn a_frame_the_filter_would_refuse_refuses_the_batch() {
    let good = frame(60, 0);
    let mut buf = Vec::with_capacity(8192);
    let short = frame(MIN_FRAME, 0);
    assert_eq!(
        pack(&[&good, &short[..MIN_FRAME - 1]], 1, &mut buf),
        Err(PackError::Frame(1))
    );
    let long = vec![0u8; MAX_FRAME + 1];
    assert_eq!(pack(&[&long], 1, &mut buf), Err(PackError::Frame(0)));
    let mut arp = frame(60, 0);
    arp[13] = 0x06;
    assert_eq!(pack(&[&arp], 1, &mut buf), Err(PackError::Frame(0)));
    let mut v6 = frame(60, 0);
    v6[14] = 0x60;
    assert_eq!(pack(&[&v6], 1, &mut buf), Err(PackError::Frame(0)));
    let mut ihl = frame(60, 0);
    ihl[14] = 0x44;
    assert_eq!(pack(&[&ihl], 1, &mut buf), Err(PackError::Frame(0)));
}

#[test]
fn the_accepted_count_marks_the_first_frames_only() {
    let mut sent = [false, true, false, true];
    mark(2, &mut sent);
    assert_eq!(sent, [true, true, false, false]);
    mark(0, &mut sent);
    assert_eq!(sent, [false; 4]);
    mark(9, &mut sent);
    assert_eq!(sent, [true; 4]);
}

// A re-enumerated NIC gets a new IfIndex: the filter answers every batch
// with "not found" and all audio falls back to the sockets (PR #4 review).
// The sender then looks the adapter up again, at most once a second.
#[test]
fn a_lost_adapter_is_looked_up_again_at_most_once_a_second() {
    let mut gate = Relookup::default();
    assert!(gate.due(10_000), "the first miss looks it up");
    assert!(!gate.due(10_500), "not again within a second");
    assert!(!gate.due(10_999));
    assert!(gate.due(11_000), "a second later again");
    assert!(!gate.due(11_001));
}
