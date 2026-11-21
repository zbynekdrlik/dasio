use super::*;

const RATE: u32 = 96_000;
const HOST: usize = 128;
const PUMP: usize = 256;

fn cfg(channels: usize) -> Config {
    Config {
        name: "master".into(),
        block: HOST,
        rx: (0..channels).map(|i| (i, format!("ch {i}"))).collect(),
        tone: vec![],
    }
}

/// A deterministic noise-like signal around -20 dBFS: never exactly zero,
/// never a repeat of the sample a block before.
fn signal(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let v = ((s >> 33) as f64 / (1u64 << 31) as f64) - 0.5;
            let v = v * 0.2;
            if v == 0.0 { 1e-4 } else { v }
        })
        .collect()
}

/// Runs `chans` (one Vec per watched channel, equal lengths) through a
/// watch in pump blocks from position `start`; the breaks it pushed.
fn run(chans: &[Vec<f64>], start: u64) -> Vec<Break> {
    let events = Arc::new(Events::default());
    let mut w = Watch::new(&cfg(chans.len()), RATE, Arc::clone(&events));
    let n = chans[0].len();
    let mut out = vec![0.0; PUMP];
    let mut pos = 0;
    while pos < n {
        let len = PUMP.min(n - pos);
        let mut input = Vec::with_capacity(len * chans.len());
        for c in chans {
            input.extend_from_slice(&c[pos..pos + len]);
        }
        let block = Block::new(len, &input, &mut out[..len]).at(start + pos as u64);
        w.check_block(&block);
        pos += len;
    }
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(events.drain(&mut tail, &mut got), 0);
    got
}

#[test]
fn clean_signal_has_no_break() {
    assert_eq!(run(&[signal(96_000, 1)], 0), vec![]);
}

#[test]
fn a_host_block_of_zeros_inside_signal_is_one_zero_break() {
    let mut s = signal(48_000, 2);
    s[10_000..10_000 + HOST].fill(0.0);
    assert_eq!(
        run(&[s], 1_000),
        vec![Break {
            channel: 0,
            kind: Kind::Zero,
            at: 11_000,
            len: HOST as u32
        }]
    );
}

#[test]
fn two_host_blocks_of_zeros_are_one_break_of_their_length() {
    let mut s = signal(48_000, 3);
    s[20_000..20_000 + 2 * HOST].fill(0.0);
    assert_eq!(
        run(&[s], 0),
        vec![Break {
            channel: 0,
            kind: Kind::Zero,
            at: 20_000,
            len: 2 * HOST as u32
        }]
    );
}

#[test]
fn zeros_within_the_tolerance_count_and_beyond_it_do_not() {
    for (len, counted) in [
        (HOST - TOLERANCE, true),
        (HOST + TOLERANCE, true),
        (HOST - TOLERANCE - 1, false),
        (HOST + TOLERANCE + 1, false),
        (50, false),
        ((MAX_BLOCKS + 1) * HOST, false),
        (MAX_BLOCKS * HOST, true),
    ] {
        let mut s = signal(48_000, 4);
        s[5_000..5_000 + len].fill(0.0);
        let got = run(&[s], 0);
        assert_eq!(
            got.len(),
            usize::from(counted),
            "zero run of {len}: {got:?}"
        );
        if counted {
            assert_eq!(got[0].len, len as u32);
            assert_eq!(got[0].at, 5_000);
        }
    }
}

#[test]
fn a_long_mute_then_signal_is_no_break() {
    // A whole number of blocks, but far more than MAX_BLOCKS.
    let mut s = signal(96_000 * 3, 5);
    s[1_000..1_000 + 1_500 * HOST].fill(0.0);
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn zeros_on_a_quiet_channel_are_no_break() {
    // Peaks under -72 dBFS: under the floor.
    let mut s: Vec<f64> = signal(48_000, 6).iter().map(|x| x * 0.0025).collect();
    s[9_000..9_000 + HOST].fill(0.0);
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn the_floor_is_judged_on_the_peak_before_the_zeros() {
    // Loud before, then 1 s of a quiet stretch still above the floor
    // (-50 dBFS), then the hole: it counts.
    let mut s = signal(96_000 * 2, 7);
    for x in &mut s[10_000..106_000] {
        *x *= 0.03;
    }
    s[100_000..100_000 + HOST].fill(0.0);
    assert_eq!(run(&[s.clone()], 0).len(), 1);
    // The same stretch under -72 dBFS: the peak decayed under the floor.
    for x in &mut s[10_000..100_000] {
        *x *= 0.083;
    }
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn an_exact_repeat_of_the_block_before_is_one_repeat_break() {
    let mut s = signal(48_000, 8);
    let (a, b) = (30_000, 30_000 + HOST);
    let copy: Vec<f64> = s[a..b].to_vec();
    s[b..b + HOST].copy_from_slice(&copy);
    assert_eq!(
        run(&[s], 0),
        vec![Break {
            channel: 0,
            kind: Kind::Repeat,
            at: b as u64,
            len: HOST as u32
        }]
    );
}

#[test]
fn a_short_repeat_is_no_break() {
    let mut s = signal(48_000, 9);
    let len = HOST - TOLERANCE - 1;
    let copy: Vec<f64> = s[30_000..30_000 + len].to_vec();
    s[30_000 + HOST..30_000 + HOST + len].copy_from_slice(&copy);
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn a_held_constant_value_is_no_repeat() {
    let mut s = signal(48_000, 10);
    s[12_000..12_000 + 4 * HOST].fill(0.25);
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn a_jump_in_the_media_position_starts_the_channel_again() {
    // A block of zeros that straddles a skipped stretch is not read as one
    // run: the watch forgets at the jump.
    let events = Arc::new(Events::default());
    let mut w = Watch::new(&cfg(1), RATE, Arc::clone(&events));
    let mut out = vec![0.0; PUMP];
    let mut first = signal(PUMP, 11);
    first[PUMP - 64..].fill(0.0);
    w.check_block(&Block::new(PUMP, &first, &mut out).at(0));
    let mut second = signal(PUMP, 12);
    second[..64].fill(0.0);
    w.check_block(&Block::new(PUMP, &second, &mut out).at(10 * PUMP as u64));
    let mut tail = 0;
    let mut got = Vec::new();
    events.drain(&mut tail, &mut got);
    assert_eq!(got, vec![]);
}

#[test]
fn a_block_with_no_media_time_follows_the_last_one() {
    let events = Arc::new(Events::default());
    let mut w = Watch::new(&cfg(1), RATE, Arc::clone(&events));
    let mut out = vec![0.0; PUMP];
    let s = signal(PUMP * 40, 13);
    for (i, chunk) in s.chunks(PUMP).enumerate() {
        let mut c = chunk.to_vec();
        if i == 30 {
            c[100..100 + HOST].fill(0.0);
        }
        w.check_block(&Block::new(PUMP, &c, &mut out));
    }
    let mut tail = 0;
    let mut got = Vec::new();
    events.drain(&mut tail, &mut got);
    assert_eq!(
        got,
        vec![Break {
            channel: 0,
            kind: Kind::Zero,
            at: 30 * PUMP as u64 + 100,
            len: HOST as u32
        }]
    );
}

#[test]
fn breaks_on_two_channels_in_one_host_block_are_one_group() {
    let mut a = signal(48_000, 14);
    let mut b = signal(48_000, 15);
    let c = signal(48_000, 16);
    a[7_000..7_000 + HOST].fill(0.0);
    b[7_002..7_002 + HOST].fill(0.0);
    let got = run(&[a, b, c], 0);
    assert_eq!(got.len(), 2);
    assert_eq!(
        group(&got, HOST),
        vec![Group {
            kind: Kind::Zero,
            at: 7_000,
            len: HOST as u32,
            channels: vec![0, 1]
        }]
    );
}

#[test]
fn breaks_far_apart_or_of_another_kind_are_their_own_groups() {
    let z = |channel, at| Break {
        channel,
        kind: Kind::Zero,
        at,
        len: 128,
    };
    let r = Break {
        channel: 2,
        kind: Kind::Repeat,
        at: 1_010,
        len: 130,
    };
    let groups = group(
        &[z(1, 5_000), z(0, 1_000), r, z(1, 1_000 + HOST as u64)],
        HOST,
    );
    assert_eq!(
        groups,
        vec![
            Group {
                kind: Kind::Zero,
                at: 1_000,
                len: 128,
                channels: vec![0, 1]
            },
            Group {
                kind: Kind::Repeat,
                at: 1_010,
                len: 130,
                channels: vec![2]
            },
            Group {
                kind: Kind::Zero,
                at: 5_000,
                len: 128,
                channels: vec![1]
            },
        ]
    );
    // One sample further is another group.
    assert_eq!(
        group(&[z(0, 1_000), z(1, 1_001 + HOST as u64)], HOST).len(),
        2
    );
}

#[test]
fn a_group_takes_the_longest_break_lists_each_channel_once_ascending() {
    let b = |channel, at, len| Break {
        channel,
        kind: Kind::Zero,
        at,
        len,
    };
    assert_eq!(
        group(
            &[b(1, 2_000, 128), b(0, 2_003, 130), b(1, 2_050, 126)],
            HOST
        ),
        vec![Group {
            kind: Kind::Zero,
            at: 2_000,
            len: 130,
            channels: vec![0, 1]
        }]
    );
}

#[test]
fn the_ring_counts_what_it_lost_and_keeps_the_newest() {
    let events = Events::default();
    let b = |at| Break {
        channel: 3,
        kind: Kind::Repeat,
        at,
        len: 129,
    };
    for at in 0..(CAPACITY as u64 + 10) {
        events.push(b(at));
    }
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(events.drain(&mut tail, &mut got), 10);
    assert_eq!(got.len(), CAPACITY);
    assert_eq!(got[0], b(10));
    assert_eq!(*got.last().unwrap(), b(CAPACITY as u64 + 9));
    assert_eq!(tail, CAPACITY as u64 + 10);
    // Nothing new: nothing out.
    let mut again = Vec::new();
    assert_eq!(events.drain(&mut tail, &mut again), 0);
    assert!(again.is_empty());
    events.push(b(7));
    events.drain(&mut tail, &mut again);
    assert_eq!(again, vec![b(7)]);
}

#[test]
fn a_break_round_trips_through_a_slot() {
    for b in [
        Break {
            channel: 0,
            kind: Kind::Zero,
            at: 0,
            len: 0,
        },
        Break {
            channel: 15,
            kind: Kind::Repeat,
            at: u64::MAX,
            len: u32::MAX,
        },
        Break {
            channel: 7,
            kind: Kind::Zero,
            at: 123_456_789,
            len: 1024,
        },
    ] {
        let events = Events::default();
        events.push(b);
        let mut tail = 0;
        let mut got = Vec::new();
        events.drain(&mut tail, &mut got);
        assert_eq!(got, vec![b]);
    }
}

#[test]
fn watched_checks_the_block_and_hands_it_on() {
    struct Count(usize, usize);
    impl Process for Count {
        fn process(&mut self, _: &mut Block<'_>) {
            self.0 += 1;
        }
        fn discontinuity(&mut self) {
            self.1 += 1;
        }
    }
    let events = Arc::new(Events::default());
    let mut p = Watched::new(
        Count(0, 0),
        Some(Watch::new(&cfg(1), RATE, Arc::clone(&events))),
    );
    let mut s = signal(PUMP * 2, 17);
    s[PUMP..PUMP + HOST].fill(0.0);
    let mut out = vec![0.0; PUMP];
    p.process(&mut Block::new(PUMP, &s[..PUMP], &mut out).at(0));
    p.process(&mut Block::new(PUMP, &s[PUMP..], &mut out).at(PUMP as u64));
    p.discontinuity();
    assert_eq!((p.inner.0, p.inner.1), (2, 1));
    let mut tail = 0;
    let mut got = Vec::new();
    events.drain(&mut tail, &mut got);
    assert_eq!(got.len(), 1);
    // Without a watch the block still goes on.
    let mut bare = Watched::new(Count(0, 0), None);
    bare.process(&mut Block::new(PUMP, &s[..PUMP], &mut out).at(0));
    assert_eq!(bare.inner.0, 1);
}

#[test]
fn a_discontinuity_forgets_a_run_in_progress() {
    let events = Arc::new(Events::default());
    let mut p = Watched::new(
        crate::miss_watch::tests::Nothing,
        Some(Watch::new(&cfg(1), RATE, Arc::clone(&events))),
    );
    let mut out = vec![0.0; PUMP];
    let mut a = signal(PUMP, 18);
    a[PUMP - 64..].fill(0.0);
    p.process(&mut Block::new(PUMP, &a, &mut out).at(0));
    p.discontinuity();
    let mut b = signal(PUMP, 19);
    b[..64].fill(0.0);
    p.process(&mut Block::new(PUMP, &b, &mut out).at(PUMP as u64));
    let mut tail = 0;
    let mut got = Vec::new();
    events.drain(&mut tail, &mut got);
    assert_eq!(got, vec![]);
}

struct Nothing;
impl Process for Nothing {
    fn process(&mut self, _: &mut Block<'_>) {}
}

#[test]
fn whole_blocks_takes_one_to_max_blocks_within_the_tolerance() {
    assert!(!whole_blocks(0, HOST));
    assert!(!whole_blocks(HOST / 2 - 1, HOST));
    assert!(whole_blocks(HOST, HOST));
    assert!(whole_blocks(3 * HOST + TOLERANCE, HOST));
    assert!(!whole_blocks(3 * HOST + TOLERANCE + 1, HOST));
    assert!(whole_blocks(3 * HOST - TOLERANCE, HOST));
    assert!(!whole_blocks(3 * HOST - TOLERANCE - 1, HOST));
    assert!(!whole_blocks((MAX_BLOCKS + 1) * HOST, HOST));
}

#[test]
fn age_is_the_distance_back_in_ms() {
    assert_eq!(age_ms(96_000, 96_000 * 3, RATE), 2_000);
    assert_eq!(age_ms(96_000 * 3 - 96, 96_000 * 3, RATE), 1);
    assert_eq!(age_ms(500, 100, RATE), 0);
}

fn names() -> Vec<String> {
    ["Hand 1 SNV", "CG SNV L", "CG SNV R", "Ruchy 1"]
        .map(String::from)
        .to_vec()
}

#[test]
fn the_config_names_channels_by_the_device_s_rx_names() {
    let c = config(
        "name = \"master\"\nblock = 128\nrx = [\"CG SNV L\", \"Hand 1 SNV\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!(
        c,
        Config {
            name: "master".into(),
            block: 128,
            rx: vec![(1, "CG SNV L".into()), (0, "Hand 1 SNV".into())],
            tone: vec![],
        }
    );
}

#[test]
fn the_config_refuses_what_it_cannot_watch_and_names_the_key() {
    let n = names();
    for (text, needle) in [
        (
            "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\nextra = 1\n",
            "unknown key `extra`",
        ),
        ("block = 128\nrx = [\"Hand 1 SNV\"]\n", "`name`"),
        (
            "name = \" \"\nblock = 128\nrx = [\"Hand 1 SNV\"]\n",
            "`name`",
        ),
        ("name = \"m\"\nrx = [\"Hand 1 SNV\"]\n", "`block` must be"),
        (
            "name = \"m\"\nblock = 15\nrx = [\"Hand 1 SNV\"]\n",
            "`block` 15",
        ),
        (
            "name = \"m\"\nblock = 8193\nrx = [\"Hand 1 SNV\"]\n",
            "`block` 8193",
        ),
        ("name = \"m\"\nblock = 128\n", "`rx` must be a list"),
        ("name = \"m\"\nblock = 128\nrx = []\n", "1 to 16"),
        ("name = \"m\"\nblock = 128\nrx = [1]\n", "strings"),
        (
            "name = \"m\"\nblock = 128\nrx = [\"PETA\"]\n",
            "\"PETA\", which is no RX channel",
        ),
        (
            "name = \"m\"\nblock = 128\nrx = [\"Ruchy 1\", \"Ruchy 1\"]\n",
            "twice",
        ),
        ("not toml", "watch.toml"),
    ] {
        let e = config(text, &n).unwrap_err();
        assert!(e.contains(needle), "{text:?}: {e}");
        assert!(e.starts_with(FILE), "{e}");
    }
    assert!(config("name = \"m\"\nblock = 16\nrx = [\"Ruchy 1\"]\n", &n).is_ok());
    assert!(config("name = \"m\"\nblock = 8192\nrx = [\"Ruchy 1\"]\n", &n).is_ok());
    let many = (0..17)
        .map(|i| format!("\"c{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let n17: Vec<String> = (0..17).map(|i| format!("c{i}")).collect();
    let e = config(&format!("name = \"m\"\nblock = 128\nrx = [{many}]\n"), &n17).unwrap_err();
    assert!(e.contains("1 to 16"), "{e}");
}

#[test]
fn kind_names_are_the_log_s_words() {
    assert_eq!(Kind::Zero.name(), "zero");
    assert_eq!(Kind::Repeat.name(), "repeat");
}

#[test]
fn loud_is_strictly_above_the_floor() {
    assert!(!loud(FLOOR));
    assert!(loud(FLOOR * (1.0 + 1e-9)));
    assert!(!loud(0.0));
}

#[test]
fn a_repeat_has_a_block_less_the_tolerance_to_max_blocks() {
    assert!(repeat_length(HOST - TOLERANCE, HOST));
    assert!(!repeat_length(HOST - TOLERANCE - 1, HOST));
    assert!(repeat_length(3 * HOST, HOST));
    assert!(repeat_length(MAX_BLOCKS * HOST + TOLERANCE, HOST));
    assert!(!repeat_length(MAX_BLOCKS * HOST + TOLERANCE + 1, HOST));
}

#[test]
fn a_signal_periodic_in_the_block_is_no_repeat() {
    // A bit-exact tone of period 64 (1.5 kHz at 96 kHz) for 1 s inside
    // noise: every sample equals the one a block before, for far longer
    // than MAX_BLOCKS.
    let mut s = signal(96_000 * 2, 22);
    let table: Vec<f64> = (0..64)
        .map(|i| 0.3 * (i as f64 * std::f64::consts::TAU / 64.0).sin() + 1e-3)
        .collect();
    for (k, x) in s[10_000..106_000].iter_mut().enumerate() {
        *x = table[k % 64];
    }
    assert_eq!(run(&[s], 0), vec![]);
}

#[test]
fn a_held_value_after_a_repeat_on_the_same_channel_is_no_break() {
    let mut s = signal(96_000, 23);
    let copy: Vec<f64> = s[30_000..30_000 + HOST].to_vec();
    s[30_000 + HOST..30_000 + 2 * HOST].copy_from_slice(&copy);
    s[40_000..40_000 + 4 * HOST].fill(0.25);
    assert_eq!(
        run(&[s], 0),
        vec![Break {
            channel: 0,
            kind: Kind::Repeat,
            at: 30_000 + HOST as u64,
            len: HOST as u32
        }]
    );
}

#[test]
fn a_read_counts_the_slots_the_writer_reached_again_as_lost() {
    let c = CAPACITY as u64;
    // Nothing pushed during the read: nothing lost.
    assert_eq!(overwritten(10, 10 + c, 10 + c), 0);
    assert_eq!(overwritten(0, 5, 5), 0);
    // Three pushed during a full read: its three oldest are suspect.
    assert_eq!(overwritten(10, 10 + c, 13 + c), 3);
    // Never more than were read.
    assert_eq!(overwritten(10, 12, 12 + 5 * c), 2);
    // A read of a few with room left: none.
    assert_eq!(overwritten(100, 110, 120), 0);
}

#[test]
fn a_repeat_of_a_block_less_the_tolerance_counts() {
    let mut s = signal(48_000, 20);
    let len = HOST - TOLERANCE;
    let copy: Vec<f64> = s[30_000..30_000 + len].to_vec();
    s[30_000 + HOST..30_000 + HOST + len].copy_from_slice(&copy);
    assert_eq!(
        run(&[s], 0),
        vec![Break {
            channel: 0,
            kind: Kind::Repeat,
            at: 30_000 + HOST as u64,
            len: len as u32
        }]
    );
}

#[test]
fn the_peak_decays_with_its_time_constant() {
    // One loud sample, then tiny values, then a host block of zeros: after
    // one time constant the peak is 0.5/e (counts); after 20 it is under
    // the floor (does not).
    let tau = (PEAK_HOLD_S * f64::from(RATE)) as usize;
    // 0.5 e^-5 = 3.4e-3 counts; 0.5 e^-7 = 4.6e-4 is under the floor.
    for (gap, counted) in [
        (tau, true),
        (5 * tau, true),
        (7 * tau, false),
        (20 * tau, false),
    ] {
        let mut s = vec![1e-6; 1_000 + gap + HOST + 1_000];
        s[999] = 0.5;
        s[1_000 + gap..1_000 + gap + HOST].fill(0.0);
        let got = run(&[s], 0);
        assert_eq!(got.len(), usize::from(counted), "gap {gap}: {got:?}");
    }
}

#[test]
fn the_config_takes_sixteen_channels() {
    let names: Vec<String> = (0..16).map(|i| format!("c{i}")).collect();
    let list = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let c = config(
        &format!("name = \"m\"\nblock = 128\nrx = [{list}]\n"),
        &names,
    )
    .unwrap();
    assert_eq!(c.rx.len(), 16);
    assert_eq!(c.rx[15], (15, "c15".to_string()));
}

#[test]
fn setup_reads_the_instance_folder_s_file_or_nothing() {
    let dir = std::env::temp_dir().join(format!("dasio-miss-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join(FILE));
    assert_eq!(setup(&dir, &names()), Ok(None));
    std::fs::write(
        dir.join(FILE),
        "name = \"master\"\nblock = 128\nrx = [\"Ruchy 1\"]\n",
    )
    .unwrap();
    assert_eq!(
        setup(&dir, &names()),
        Ok(Some(Config {
            name: "master".into(),
            block: 128,
            rx: vec![(3, "Ruchy 1".into())],
            tone: vec![],
        }))
    );
    std::fs::write(
        dir.join(FILE),
        "name = \"master\"\nblock = 128\nrx = [\"nope\"]\n",
    )
    .unwrap();
    let e = setup(&dir, &names()).unwrap_err();
    assert!(e.contains("\"nope\", which is no RX channel"), "{e}");
    // A folder where the file should be: an error that names it.
    std::fs::remove_file(dir.join(FILE)).unwrap();
    std::fs::create_dir(dir.join(FILE)).unwrap();
    let e = setup(&dir, &names()).unwrap_err();
    assert!(e.contains(FILE), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn channel_names_are_the_config_s_words() {
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"CG SNV L\", \"Hand 1 SNV\", \"Ruchy 1\"]\n",
        &names(),
    )
    .unwrap();
    let g = Group {
        kind: Kind::Zero,
        at: 0,
        len: 128,
        channels: vec![0, 2],
    };
    assert_eq!(channel_names(&g, &c), "CG SNV L, Ruchy 1");
    let g = Group {
        kind: Kind::Zero,
        at: 0,
        len: 128,
        channels: vec![1],
    };
    assert_eq!(channel_names(&g, &c), "Hand 1 SNV");
}

#[test]
fn a_setup_s_watches_share_its_ring() {
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\n",
        &names(),
    )
    .unwrap();
    let set = Setup::new(c);
    let mut w = set.watch(RATE);
    let mut s = signal(PUMP * 2, 21);
    s[PUMP + 10..PUMP + 10 + HOST].fill(0.0);
    let mut out = vec![0.0; PUMP];
    // The device's channel 0 is "Hand 1 SNV"; give the block 4 channels.
    let mut input = vec![0.0; PUMP * 4];
    for (k, part) in s.chunks(PUMP).enumerate() {
        input[..PUMP].copy_from_slice(part);
        for x in &mut input[PUMP..] {
            *x = 0.3;
        }
        w.check_block(&Block::new(PUMP, &input, &mut out).at((k * PUMP) as u64));
    }
    let mut tail = 0;
    let mut got = Vec::new();
    set.events.drain(&mut tail, &mut got);
    assert_eq!(
        got,
        vec![Break {
            channel: 0,
            kind: Kind::Zero,
            at: PUMP as u64 + 10,
            len: HOST as u32
        }]
    );
}

#[test]
fn a_drain_drops_and_counts_what_the_writer_overwrote_meanwhile() {
    let events = Events::default();
    let b = |at| Break {
        channel: 1,
        kind: Kind::Zero,
        at,
        len: 128,
    };
    for at in 0..CAPACITY as u64 {
        events.push(b(at));
    }
    let mut tail = 0;
    let mut got = Vec::new();
    // Three more pushed while the full ring is read: its three oldest
    // slots are the new ones now, so they go, counted as lost.
    let lost = events.drain_with(&mut tail, &mut got, || {
        for at in 1_000..1_003 {
            events.push(b(at));
        }
    });
    assert_eq!(lost, 3);
    assert_eq!(got.len(), CAPACITY - 3);
    assert_eq!(got[0], b(3));
    assert_eq!(tail, CAPACITY as u64);
    // The next drain gets the three pushed meanwhile.
    let mut next = Vec::new();
    assert_eq!(events.drain(&mut tail, &mut next), 0);
    assert_eq!(next, vec![b(1_000), b(1_001), b(1_002)]);
}

#[test]
fn the_config_reads_tone_channels_next_to_rx() {
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\ntone = [\"CG SNV R\", \"CG SNV L\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!(c.rx, vec![(0, "Hand 1 SNV".to_string())]);
    assert_eq!(
        c.tone,
        vec![(2, "CG SNV R".to_string()), (1, "CG SNV L".to_string())]
    );
    // A channel may be in both lists.
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Ruchy 1\"]\ntone = [\"Ruchy 1\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!((c.rx.len(), c.tone.len()), (1, 1));
}

#[test]
fn a_tone_only_config_needs_no_rx() {
    for text in [
        "name = \"m\"\nblock = 128\ntone = [\"Ruchy 1\"]\n",
        "name = \"m\"\nblock = 128\nrx = []\ntone = [\"Ruchy 1\"]\n",
    ] {
        let c = config(text, &names()).unwrap();
        assert_eq!(
            (c.rx, c.tone),
            (vec![], vec![(3, "Ruchy 1".to_string())]),
            "{text}"
        );
    }
}

#[test]
fn the_config_refuses_a_tone_list_it_cannot_watch() {
    let rx = "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\n";
    for (tone, e) in [
        ("tone = []", "`tone` must name 1 to 16 channels"),
        (
            "tone = \"Ruchy 1\"",
            "`tone` must be a list of RX channel names",
        ),
        ("tone = [2]", "`tone` entries must be strings"),
        (
            "tone = [\"PETA\"]",
            "`tone` names \"PETA\", which is no RX channel of this device",
        ),
        (
            "tone = [\"Ruchy 1\", \"Ruchy 1\"]",
            "`tone` names \"Ruchy 1\" twice",
        ),
    ] {
        for text in [
            format!("{rx}{tone}\n"),
            format!("name = \"m\"\nblock = 128\n{tone}\n"),
        ] {
            assert_eq!(
                config(&text, &names()).unwrap_err(),
                format!("{FILE}: {e}"),
                "{text}"
            );
        }
    }
    // Neither list: `rx` is missing.
    assert_eq!(
        config("name = \"m\"\nblock = 128\n", &names()).unwrap_err(),
        format!("{FILE}: `rx` must be a list of RX channel names")
    );
}

#[test]
fn the_tone_list_takes_sixteen_channels_and_rx_up_to_sixteen_beside_it() {
    let n17: Vec<String> = (0..17).map(|i| format!("c{i}")).collect();
    let list = |k: usize| {
        n17[..k]
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let c = config(
        &format!("name = \"m\"\nblock = 128\ntone = [{}]\n", list(16)),
        &n17,
    )
    .unwrap();
    assert_eq!((c.tone.len(), c.tone[15].0), (16, 15));
    assert_eq!(
        config(
            &format!("name = \"m\"\nblock = 128\ntone = [{}]\n", list(17)),
            &n17
        )
        .unwrap_err(),
        format!("{FILE}: `tone` must name 1 to 16 channels")
    );
    assert_eq!(
        config(
            &format!(
                "name = \"m\"\nblock = 128\nrx = [{}]\ntone = [\"c0\"]\n",
                list(17)
            ),
            &n17
        )
        .unwrap_err(),
        format!("{FILE}: `rx` must name 0 to 16 channels")
    );
}

#[test]
fn a_setup_starts_its_tone_figures_one_per_tone_channel() {
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\ntone = [\"CG SNV L\", \"CG SNV R\"]\n",
        &names(),
    )
    .unwrap();
    let set = Setup::new(c);
    assert_eq!(set.tones.take(1).checked, 0);
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(set.tones.events.drain(&mut tail, &mut got), 0);
    assert!(got.is_empty());
}
