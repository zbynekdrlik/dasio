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

// The enter fields use what was padding: the header keeps its size, so a
// driver and a device of either age share one layout (VERSION unchanged).
#[test]
fn the_header_keeps_its_size() {
    assert_eq!(std::mem::size_of::<Header>(), 128);
    assert_eq!(std::mem::offset_of!(Header, enter_seq), 104);
    assert_eq!(std::mem::offset_of!(Header, enter_ns), 112);
}

#[test]
fn an_entered_block_reads_back_its_time() {
    let h = Header::default();
    // An older driver never writes it.
    assert_eq!(h.entered(1), None);
    h.enter(7, 1_234);
    assert_eq!(h.entered(7), Some(1_234));
    // Another block's time is never paired with this seq.
    assert_eq!(h.entered(6), None);
    assert_eq!(h.entered(8), None);
    h.enter(8, 5_678);
    assert_eq!(h.entered(7), None);
    assert_eq!(h.entered(8), Some(5_678));
    // Seq 0 is never a block.
    h.enter(0, 9);
    assert_eq!(h.entered(0), None);
}

#[test]
fn a_half_written_enter_is_not_read() {
    let h = Header::default();
    h.enter(3, 100);
    // The driver mid-write: seq cleared, time not yet paired.
    h.enter_seq.store(0, Ordering::Release);
    h.enter_ns.store(200, Ordering::Release);
    assert_eq!(h.entered(3), None);
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
// another block's time while the driver keeps writing. The reader ends when
// the writer has, so a driver that never writes fails here, never hangs.
#[test]
fn a_racing_reader_never_pairs_a_seq_with_another_time() {
    use std::sync::atomic::AtomicBool;
    let h = Header::default();
    let done = AtomicBool::new(false);
    const N: u64 = 200_000;
    std::thread::scope(|s| {
        s.spawn(|| {
            for i in 1..=N {
                h.enter(i, i * 1000);
            }
            done.store(true, Ordering::Release);
        });
        let mut paired = 0u64;
        loop {
            let finished = done.load(Ordering::Acquire);
            let seq = h.enter_seq.load(Ordering::Acquire);
            if seq != 0
                && let Some(ns) = h.entered(seq)
            {
                assert_eq!(ns, seq * 1000, "seq {seq} read another block's time");
                paired += 1;
            }
            if finished {
                break;
            }
        }
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

// dasio#22: a slow block's ack is followed at once by the driver entering
// the next block (it is already due). The slow block's callback start must
// still be readable then, or the split drops exactly the slow blocks.
#[test]
fn a_slow_block_keeps_its_start_after_the_next_block_enters() {
    let h = Header::default();
    h.enter(10, 10_000);
    h.enter(11, 11_000);
    assert_eq!(h.entered(10), Some(10_000));
    assert_eq!(h.entered(11), Some(11_000));
}
