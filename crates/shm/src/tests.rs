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
