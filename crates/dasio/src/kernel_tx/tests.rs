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

/// A v3 record as the filter writes it: 25 little-endian u64s.
fn record(if_index: u64, set: &[(usize, u64)]) -> Vec<u8> {
    let mut w = [0u64; 25];
    w[0] = 3;
    w[1] = if_index;
    for &(i, v) in set {
        w[i] = v;
    }
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn the_status_ioctl_code_is_filteruser_h_s() {
    // _NDIS_CONTROL_CODE(18, METHOD_BUFFERED): FILE_DEVICE_PHYSICAL_NETCARD, FILE_ANY_ACCESS.
    assert_eq!(IOCTL_DASIO_KTX_STATUS, 0x0017_0048);
}

#[test]
fn a_status_reply_reads_each_adapter_s_record_in_order() {
    let mut reply = record(
        7,
        &[
            (10, 5),
            (11, 2),
            (17, 1000),
            (18, 3),
            (20, 200),
            (21, 150),
            (22, 1),
            (23, 0xC000_009A),
        ],
    );
    reply.extend(record(12, &[(17, 9)]));
    let s = parse_status(&reply);
    assert_eq!(s.len(), 2);
    assert_eq!(
        s[0],
        KtxStatus {
            version: 3,
            if_index: 7,
            failed: 5,
            out: 2,
            tx_frames: 1000,
            tx_no_slot: 3,
            tx_max_out: 200,
            max_out: 150,
            fail_events: 1,
            last_fail_status: 0xC000_009A,
        }
    );
    assert_eq!(s[1].if_index, 12);
    assert_eq!(s[1].tx_frames, 9);
}

#[test]
fn an_older_record_reads_its_own_size_and_zero_for_the_newer_fields() {
    // Version 2: 20 fields, no TxMaxOut / MaxOut / FailEvents.
    let mut w = [0u64; 20];
    w[0] = 2;
    w[1] = 4;
    w[10] = 6;
    w[18] = 8;
    let mut reply: Vec<u8> = w.iter().flat_map(|v| v.to_le_bytes()).collect();
    reply.extend(record(5, &[(22, 2)]));
    let s = parse_status(&reply);
    assert_eq!(s.len(), 2);
    assert_eq!(
        (s[0].version, s[0].if_index, s[0].failed, s[0].tx_no_slot),
        (2, 4, 6, 8)
    );
    assert_eq!((s[0].fail_events, s[0].max_out), (0, 0));
    assert_eq!((s[1].if_index, s[1].fail_events), (5, 2));
}

#[test]
fn an_unknown_version_or_a_short_record_ends_the_walk() {
    let mut reply = record(1, &[]);
    let mut odd = record(2, &[]);
    odd[0] = 9;
    reply.extend(odd);
    assert_eq!(parse_status(&reply).len(), 1);
    let cut = record(3, &[]);
    assert!(parse_status(&cut[..cut.len() - 1]).is_empty());
    assert!(parse_status(&[]).is_empty());
}

fn status(if_index: u32, frames: u64, no_slot: u64, failed: u64, events: u64) -> KtxStatus {
    KtxStatus {
        version: 3,
        if_index,
        tx_frames: frames,
        tx_no_slot: no_slot,
        failed,
        fail_events: events,
        max_out: 40,
        tx_max_out: 200,
        out: 1,
        last_fail_status: 0,
    }
}

#[test]
fn the_watch_reports_the_window_between_two_readings_of_its_adapter() {
    let mut w = StatusWatch::default();
    let other = status(9, 1, 1, 1, 1);
    assert_eq!(w.step(&[other, status(7, 100, 2, 1, 1)], 7), None);
    let r = w.step(&[status(7, 1600, 5, 4, 2), other], 7).unwrap();
    assert_eq!(
        (r.frames, r.no_slot, r.failed, r.fail_events),
        (1500, 3, 3, 1)
    );
    assert_eq!((r.max_out, r.tx_max_out, r.out), (40, 200, 1));
    assert!(r.bad());
    let r = w.step(&[status(7, 2600, 5, 4, 2)], 7).unwrap();
    assert_eq!((r.frames, r.no_slot, r.failed), (1000, 0, 0));
    assert!(!r.bad());
}

#[test]
fn a_lost_adapter_or_a_restarted_filter_starts_a_new_baseline() {
    let mut w = StatusWatch::default();
    w.step(&[status(7, 100, 0, 0, 0)], 7);
    // Our adapter missing: no report, and no window across the gap.
    assert_eq!(w.step(&[status(9, 5, 0, 0, 0)], 7), None);
    assert_eq!(w.step(&[status(7, 500, 0, 0, 0)], 7), None);
    // Counters back to small values: the filter restarted.
    assert_eq!(w.step(&[status(7, 10, 0, 0, 0)], 7), None);
    let r = w.step(&[status(7, 30, 1, 0, 0)], 7).unwrap();
    assert_eq!((r.frames, r.no_slot), (20, 1));
    assert!(r.bad());
}
