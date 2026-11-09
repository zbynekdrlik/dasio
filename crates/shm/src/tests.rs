use super::*;

#[test]
fn a_shape_holds_only_the_device_s_blocks_and_channel_counts() {
    assert!(Shape::new(8, 8, 32).is_some());
    assert!(Shape::new(64, 64, 256).is_some());
    assert!(Shape::new(65, 8, 32).is_none());
    assert!(Shape::new(8, 65, 32).is_none());
    assert!(Shape::new(8, 8, 48).is_none());
}

#[test]
fn offsets_are_aligned_and_disjoint() {
    for &(rx, tx) in &[(0, 8), (8, 0), (8, 8), (64, 64)] {
        for &block in &BLOCKS {
            let s = Shape::new(rx, tx, block).unwrap();
            let len = (block * 4) as usize;
            let mut spans = Vec::new();
            for half in 0..2 {
                for (input, n) in [(true, rx), (false, tx)] {
                    for ch in 0..n {
                        let o = s.buffer_offset(half, input, ch).unwrap();
                        assert_eq!(o % ALIGN, 0, "{rx}/{tx} at {block}");
                        assert!(o >= std::mem::size_of::<Header>());
                        assert!(o + len <= s.size());
                        spans.push((o, o + len));
                    }
                }
            }
            spans.sort_unstable();
            assert!(
                spans.windows(2).all(|w| w[0].1 <= w[1].0),
                "overlap at {rx}/{tx} {block}"
            );
        }
    }
}

#[test]
fn offsets_past_the_shape_are_none() {
    let s = Shape::new(2, 3, 32).unwrap();
    assert_eq!(s.buffer_offset(2, true, 0), None);
    assert_eq!(s.buffer_offset(0, true, 2), None);
    assert_eq!(s.buffer_offset(0, false, 3), None);
    assert!(s.buffer_offset(1, false, 2).is_some());
}

#[test]
fn the_header_is_one_cache_line_aligned_block() {
    assert_eq!(std::mem::align_of::<Header>(), ALIGN);
    assert_eq!(MAGIC.to_le_bytes(), *b"DASI");
}

#[test]
fn the_size_is_the_header_then_two_halves() {
    let header = std::mem::size_of::<Header>().next_multiple_of(ALIGN);
    // 1 + 1 channels of 32 samples: two 128-byte buffers a half.
    assert_eq!(Shape::new(1, 1, 32).unwrap().size(), header + 2 * 2 * 128);
    // 16 samples: 64 bytes, already aligned; 3 + 2 channels.
    assert_eq!(Shape::new(3, 2, 16).unwrap().size(), header + 2 * 5 * 64);
}

// install.ps1 refuses to keep a held driver of another protocol: its
// number must be this crate's VERSION.
#[test]
fn the_installer_s_protocol_is_the_layout_s_version() {
    let script = include_str!("../../../scripts/install.ps1");
    let want = format!("$ShmProtocol = {VERSION}");
    assert!(
        script.lines().any(|l| l.trim() == want),
        "scripts/install.ps1 must set {want}"
    );
}

// The ack record uses what was padding: the header keeps its size, so a
// driver and a device of either age share one layout (VERSION unchanged).
#[test]
fn the_header_keeps_its_size() {
    assert_eq!(std::mem::size_of::<Header>(), 128);
    assert_eq!(std::mem::offset_of!(Header, acked_seq), 104);
    assert_eq!(std::mem::offset_of!(Header, acked_entered_ns), 112);
    assert_eq!(std::mem::offset_of!(Header, driver_messages), 120);
}

// dasio#28 E16: each kind counts on its own; an older driver's 0 reads as
// none sent.
#[test]
fn the_driver_s_messages_are_counted_by_kind() {
    let h = Header::default();
    assert_eq!(h.messages(), (0, 0));
    h.note_message(DriverMessage::Reset);
    h.note_message(DriverMessage::Latencies);
    h.note_message(DriverMessage::Latencies);
    assert_eq!(h.messages(), (1, 2));
    assert_eq!(split_messages((7 << 32) | 3), (3, 7));
    assert_eq!(split_messages(u64::MAX), (u32::MAX, u32::MAX));
}

#[test]
fn an_acked_block_reads_back_its_start() {
    let h = Header::default();
    // An older driver never writes it.
    assert_eq!(h.entered(1), None);
    h.ack_block(7, 1_234);
    assert_eq!(h.entered(7), Some(1_234));
    // Another block's start is never paired with this seq.
    assert_eq!(h.entered(6), None);
    assert_eq!(h.entered(8), None);
    h.ack_block(8, 5_678);
    assert_eq!(h.entered(8), Some(5_678));
    assert_eq!(h.entered(7), None);
    // A start the driver did not know is no split.
    h.ack_block(9, 0);
    assert_eq!(h.entered(9), None);
    assert_eq!(h.ack.load(Ordering::Acquire), 9);
    // Seq 0 is never a block.
    h.ack_block(0, 9);
    assert_eq!(h.entered(0), None);
}

#[test]
fn a_half_written_record_is_not_read() {
    let h = Header::default();
    h.ack_block(3, 100);
    // The driver mid-write: seq cleared, start not yet paired.
    h.acked_seq.store(0, Ordering::Release);
    h.acked_entered_ns.store(200, Ordering::Release);
    assert_eq!(h.entered(3), None);
}

// Older drivers wrote other values here: 0.1.18 the latest entered seq with
// no mark, 0.1.21 the latest entered seq with bit 63 and one start per
// half. Theirs are never read, so a mismatch gives no split rather than a
// wrong one.
#[test]
fn an_older_driver_is_never_read() {
    let h = Header::default();
    h.acked_seq.store(10, Ordering::Release);
    h.acked_entered_ns.store(5_000, Ordering::Release);
    assert_eq!(h.entered(10), None);
    h.acked_seq.store(10 | (1 << 63), Ordering::Release);
    assert_eq!(h.entered(10), None);
}

#[test]
fn counter_ticks_turn_into_ns() {
    // 10 MHz (Windows' usual QPC rate): one tick is 100 ns.
    assert_eq!(ticks_to_ns(1, 10_000_000), 100);
    assert_eq!(ticks_to_ns(10_000_000, 10_000_000), 1_000_000_000);
    // An odd rate keeps the remainder's share.
    assert_eq!(ticks_to_ns(3, 3_000_000), 1_000);
    assert_eq!(ticks_to_ns(4, 3_000_000), 1_333);
    // Three years of a 10 MHz counter: ticks * 1e9 does not fit in a u64,
    // the result does, exactly.
    assert_eq!(
        ticks_to_ns(1_000_000_000_000_007, 10_000_000),
        100_000_000_000_000_700
    );
    // A result past u64 saturates, never wraps or panics.
    assert_eq!(ticks_to_ns(u64::MAX, 10_000_000), u64::MAX);
    assert_eq!(ticks_to_ns(u64::MAX, 1), u64::MAX);
    // A zero rate is never a division by zero.
    assert_eq!(ticks_to_ns(5, 0), 5_000_000_000);
}

// The seqlock under a real race: a reader never pairs one block's seq with
// another block's start while the driver keeps returning blocks. The
// reader ends when the writer has (or has panicked), so a broken writer
// fails here, never hangs.
#[test]
fn a_racing_reader_never_pairs_a_seq_with_another_start() {
    let h = Header::default();
    const N: u64 = 200_000;
    std::thread::scope(|s| {
        let writer = s.spawn(|| {
            for i in 1..=N {
                h.ack_block(i, i * 1000);
            }
        });
        let mut paired = 0u64;
        loop {
            let finished = writer.is_finished();
            let seq = h.ack.load(Ordering::Acquire);
            if let Some(ns) = h.entered(seq) {
                assert_eq!(ns, seq * 1000, "seq {seq} read another block's start");
                paired += 1;
            } else if seq != 0 {
                // No pairing only because the driver already writes a newer
                // record: never one behind the ack a reader sees (the record
                // is written before its ack).
                let record = h.acked_seq.load(Ordering::Acquire) & !ACKED_MARK;
                assert!(
                    record == 0 || record > seq,
                    "ack {seq} visible before its record ({record})"
                );
            }
            if finished {
                break;
            }
        }
        writer.join().expect("the writer ran through");
        assert!(paired > 0);
        assert_eq!(h.entered(N), Some(N * 1000));
    });
}

// The shared clock runs forward and in ns: a 20 ms sleep measures at least
// 20 ms (a sleep never ends early) and not absurdly more.
#[cfg(windows)]
#[test]
fn the_shared_clock_runs_forward_in_ns() {
    let a = qpc_ns();
    let b = qpc_ns();
    assert!(b >= a);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let slept = qpc_ns() - b;
    assert!(
        (20_000_000..2_000_000_000).contains(&slept),
        "a 20 ms sleep measured {slept} ns"
    );
}

// dasio#22, the biggest stalls: after a long callback the driver jumps to
// the newest block (the ones between are missed), which may share the slow
// block's half. The slow block's callback start must stay readable from
// its ack until the next block's ack, whatever block the driver enters.
#[test]
fn a_block_keeps_its_start_until_the_next_ack() {
    let h = Header::default();
    h.ack_block(10, 10_000);
    assert_eq!(h.ack.load(Ordering::Acquire), 10);
    // The driver jumps to block 14 (11-13 missed) and enters it: nothing in
    // the header changes until it returns 14.
    assert_eq!(h.entered(10), Some(10_000));
    h.ack_block(14, 14_000);
    assert_eq!(h.entered(14), Some(14_000));
    assert_eq!(h.entered(10), None);
}

// The pairing rule on its own, deterministically: a torn read (the mark
// changed between the two loads), another block's mark, an unknown start
// and seq 0 are never a pairing.
#[test]
fn a_torn_or_foreign_read_never_pairs() {
    let m = |seq: u64| seq | ACKED_MARK;
    assert_eq!(paired(5, m(5), 500, m(5)), Some(500));
    // The driver started block 6's record between the two loads.
    assert_eq!(paired(5, m(5), 600, 0), None);
    assert_eq!(paired(5, m(5), 600, m(6)), None);
    assert_eq!(paired(5, 0, 500, m(5)), None);
    // Another block's record, an older driver's value, an unknown start.
    assert_eq!(paired(5, m(4), 400, m(4)), None);
    assert_eq!(paired(5, 5, 500, 5), None);
    assert_eq!(paired(5, m(5), 0, m(5)), None);
    assert_eq!(paired(0, m(0), 1, m(0)), None);
}
