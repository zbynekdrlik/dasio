use super::*;
use crate::miss_watch::{Config, Setup};
use dstack_audio_io::Block;

const RATE: u32 = 96_000;
const LSB: f64 = 1.0 / 8_388_608.0;

fn cfg(channels: usize, block: usize) -> Config {
    Config {
        name: "master".into(),
        block,
        rx: vec![],
        tone: vec![],
        program: (0..channels).map(|i| (i, format!("prog {i}"))).collect(),
    }
}

/// Sample `i` of a signal that equals itself only a multiple of 1000
/// samples apart: 1 to 1000 steps of 24 bits, never zero, never the same
/// twice in a row.
fn s(i: usize) -> f64 {
    ((i * 7919) % 1000 + 1) as f64 * LSB
}

/// `n` samples of [`s`] from index `from`.
fn fresh(from: usize, n: usize) -> Vec<f64> {
    (from..from + n).map(s).collect()
}

/// `lead` fresh samples, then the last `lag` of them again and again for
/// `len` samples, then `tail` fresh ones.
fn with_loop(lead: usize, lag: usize, len: usize, tail: usize) -> Vec<f64> {
    let mut out = fresh(0, lead);
    for j in 0..len {
        let v = out[lead - lag + j % lag];
        out.push(v);
    }
    out.extend(fresh(lead + len + 7, tail));
    out
}

/// The loudest of `x`.
fn loudest(x: &[f64]) -> f64 {
    x.iter().fold(0.0_f64, |m, v| m.max(v.abs()))
}

/// Runs `chans` (one Vec per program channel, equal lengths) through a
/// watch of host block `block` in pump blocks of `pump`; at pump block
/// `jump.0` the media position jumps by `jump.1` samples. The loops, in
/// push order.
fn run_with(
    chans: &[Vec<f64>],
    block: usize,
    pump: usize,
    jump: Option<(usize, u64)>,
) -> Vec<Loop> {
    let set = Setup::new(cfg(chans.len(), block));
    let mut w = set.watch(RATE);
    let n = chans[0].len();
    let mut out = vec![0.0; pump];
    let (mut pos, mut start, mut k) = (0, 0u64, 0);
    while pos < n {
        let len = pump.min(n - pos);
        let mut input = Vec::with_capacity(len * chans.len());
        for c in chans {
            input.extend_from_slice(&c[pos..pos + len]);
        }
        if jump.is_some_and(|(at, _)| at == k) {
            start += jump.map_or(0, |(_, by)| by);
        }
        let b = Block::new(len, &input, &mut out[..len]).at(start);
        w.check_block(&b);
        pos += len;
        start += len as u64;
        k += 1;
    }
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(set.programs.loops.drain(&mut tail, &mut got), 0);
    got
}

/// A host block of 64 in the config, pump blocks of 256.
fn run(chans: &[Vec<f64>]) -> Vec<Loop> {
    run_with(chans, 64, 256, None)
}

fn a_loop(channel: usize, at: u64, lag: u32, len: u32, peak: f64) -> Loop {
    Loop {
        channel,
        at,
        lag,
        len,
        peak,
    }
}

#[test]
fn clean_audio_has_no_loop() {
    assert_eq!(run(&[fresh(0, 20_000)]), Vec::<Loop>::new());
}

/// The event of 24.11.2026: a ring of two host blocks of 128 (256 samples,
/// under a config block of 64) played 20 times.
#[test]
fn a_ring_of_256_played_again_is_one_loop_at_lag_256() {
    let sig = with_loop(3_000, 256, 19 * 256, 3_000);
    let want = vec![a_loop(0, 3_000, 256, 4_864, loudest(&sig[3_000..7_864]))];
    assert_eq!(want[0].peak, 998.0 * LSB);
    assert_eq!(run(std::slice::from_ref(&sig)), want);
    // The pump's block size changes nothing.
    assert_eq!(run_with(std::slice::from_ref(&sig), 64, 48, None), want);
    // A config block of the host's own 128: the ring is two of them.
    assert_eq!(run_with(&[sig], 128, 256, None), want);
}

#[test]
fn one_repeated_block_is_a_loop_at_one_block() {
    let sig = with_loop(3_000, 64, 64, 3_000);
    let want = vec![a_loop(0, 3_000, 64, 64, loudest(&sig[3_000..3_064]))];
    assert_eq!(run(&[sig]), want);
}

/// A loop of one block also repeats two and four blocks later: it is told
/// once, at the smallest lag.
#[test]
fn a_loop_is_told_once_at_its_smallest_lag() {
    let sig = with_loop(3_000, 64, 320, 3_000);
    let want = vec![a_loop(0, 3_000, 64, 320, loudest(&sig[3_000..3_320]))];
    assert_eq!(run(&[sig]), want);
}

#[test]
fn a_loop_of_two_blocks_is_told_at_two_blocks() {
    let sig = with_loop(3_000, 128, 300, 3_000);
    let want = vec![a_loop(0, 3_000, 128, 300, loudest(&sig[3_000..3_300]))];
    assert_eq!(run(&[sig]), want);
}

/// A host block that is no power of two: the ring is the next one up.
#[test]
fn a_block_of_480_loops_at_480_and_at_960() {
    let sig = with_loop(5_000, 960, 2_000, 3_000);
    let want = vec![a_loop(0, 5_000, 960, 2_000, loudest(&sig[5_000..7_000]))];
    assert_eq!(run_with(&[sig], 480, 256, None), want);
    let sig = with_loop(5_000, 480, 2_000, 3_000);
    let want = vec![a_loop(0, 5_000, 480, 2_000, loudest(&sig[5_000..7_000]))];
    assert_eq!(run_with(&[sig], 480, 256, None), want);
}

/// At least one lag less the tolerance: 60 of 64 samples are a loop, 59
/// are not.
#[test]
fn a_run_shorter_than_a_lag_less_the_tolerance_is_no_loop() {
    assert_eq!(run(&[with_loop(3_000, 64, 59, 3_000)]), Vec::<Loop>::new());
    let sig = with_loop(3_000, 64, 60, 3_000);
    let want = vec![a_loop(0, 3_000, 64, 60, loudest(&sig[3_000..3_060]))];
    assert_eq!(run(&[sig]), want);
}

#[test]
fn a_held_value_and_silence_are_no_loop() {
    let mut held = fresh(0, 3_000);
    held.extend_from_slice(&[0.25; 600]);
    held.extend(fresh(3_607, 3_000));
    assert_eq!(run(&[held]), Vec::<Loop>::new());
    let mut silent = fresh(0, 3_000);
    silent.extend_from_slice(&[0.0; 600]);
    silent.extend(fresh(3_607, 3_000));
    assert_eq!(run(&[silent]), Vec::<Loop>::new());
}

/// Digital silence with two stray 24-bit steps one lag apart repeats sample
/// for sample for a whole lag, with two changes in it: not a loop.
#[test]
fn stray_steps_in_silence_one_lag_apart_are_no_loop() {
    let mut sig = fresh(0, 3_000);
    sig.extend_from_slice(&[0.0; 600]);
    sig.extend(fresh(3_607, 3_000));
    sig[3_100] = LSB;
    sig[3_164] = LSB;
    assert_eq!(run(&[sig]), Vec::<Loop>::new());
}

/// Samples exactly zero inside the loop do not end it.
#[test]
fn a_loop_through_zero_samples_is_one_loop() {
    let mut sig = fresh(0, 3_000);
    sig[2_900] = 0.0;
    sig[2_901] = 0.0;
    for j in 0..4_864 {
        let v = sig[3_000 - 256 + j % 256];
        sig.push(v);
    }
    sig.extend(fresh(3_000 + 4_864 + 7, 3_000));
    let want = vec![a_loop(0, 3_000, 256, 4_864, loudest(&sig[3_000..7_864]))];
    assert_eq!(run(&[sig]), want);
}

/// A jump in the media position starts the check again: the run before it
/// is dropped, and the loop that goes on is told from one lag after the
/// jump (pump block 48 of 64 samples is sample 3 072, 72 samples into the
/// loop).
#[test]
fn a_discontinuity_starts_the_run_again() {
    let sig = with_loop(3_000, 64, 320, 3_000);
    let want = vec![a_loop(0, 4_136, 64, 184, loudest(&sig[3_136..3_320]))];
    assert_eq!(run_with(&[sig], 64, 64, Some((48, 1_000))), want);
}

/// Two channels that loop at the same sample: an event each, one line.
#[test]
fn two_channels_looping_together_are_one_line() {
    let a = with_loop(3_000, 256, 4_864, 3_000);
    let b: Vec<f64> = a.iter().map(|v| v * 2.0).collect();
    let (pa, pb) = (loudest(&a[3_000..7_864]), loudest(&b[3_000..7_864]));
    let got = run(&[a, b]);
    assert_eq!(
        got,
        vec![
            a_loop(0, 3_000, 256, 4_864, pa),
            a_loop(1, 3_000, 256, 4_864, pb)
        ]
    );
    assert_eq!(
        lines(&got),
        vec![Line {
            at: 3_000,
            lag: 256,
            len: 4_864,
            peak: pb,
            channels: vec![0, 1],
        }]
    );
}

fn line(at: u64, lag: u32, len: u32, peak: f64, channels: &[usize]) -> Line {
    Line {
        at,
        lag,
        len,
        peak,
        channels: channels.to_vec(),
    }
}

#[test]
fn lines_join_one_lag_within_one_sample() {
    // One sample apart, one lag: one line with the longest length and the
    // loudest peak, whatever the push order.
    let got = lines(&[
        a_loop(2, 1_001, 256, 500, 0.5),
        a_loop(0, 1_000, 256, 700, 0.25),
    ]);
    assert_eq!(got, vec![line(1_000, 256, 700, 0.5, &[0, 2])]);
    // Two samples apart: two lines.
    let got = lines(&[
        a_loop(0, 1_000, 256, 700, 0.25),
        a_loop(1, 1_002, 256, 500, 0.5),
    ]);
    assert_eq!(
        got,
        vec![
            line(1_000, 256, 700, 0.25, &[0]),
            line(1_002, 256, 500, 0.5, &[1])
        ]
    );
    // The same sample, another lag: two lines.
    let got = lines(&[
        a_loop(0, 1_000, 256, 700, 0.25),
        a_loop(1, 1_000, 128, 500, 0.5),
    ]);
    assert_eq!(
        got,
        vec![
            line(1_000, 256, 700, 0.25, &[0]),
            line(1_000, 128, 500, 0.5, &[1])
        ]
    );
    // Another lag between two channels of one lag does not split them.
    let got = lines(&[
        a_loop(0, 1_000, 256, 700, 0.25),
        a_loop(1, 1_000, 128, 500, 0.5),
        a_loop(2, 1_000, 256, 900, 0.125),
    ]);
    assert_eq!(
        got,
        vec![
            line(1_000, 256, 900, 0.25, &[0, 2]),
            line(1_000, 128, 500, 0.5, &[1])
        ]
    );
    // A channel twice in a line is named once.
    let got = lines(&[a_loop(1, 1_000, 64, 70, 0.1), a_loop(1, 1_001, 64, 80, 0.2)]);
    assert_eq!(got, vec![line(1_000, 64, 80, 0.2, &[1])]);
    assert_eq!(lines(&[]), Vec::<Line>::new());
}

#[test]
fn a_loop_survives_its_slot() {
    // The largest lag (four blocks of 8192) and a length near 2^32.
    let l = a_loop(5, 68_796_954_276, 32_768, 4_000_000_000, 0.123);
    assert_eq!(Loop::unpack(l.pack()), l);
    let l = a_loop(15, 7, 256, 4_864, 1.0);
    assert_eq!(Loop::unpack(l.pack()), l);
    let ring = LoopEvents::default();
    ring.push(l);
    let (mut tail, mut got) = (0, Vec::new());
    assert_eq!(ring.drain(&mut tail, &mut got), 0);
    assert_eq!(got, vec![l]);
}

#[test]
fn a_loop_is_a_lag_less_the_tolerance_at_least() {
    assert!(looped(60, 64));
    assert!(!looped(59, 64));
    assert!(looped(252, 256));
    assert!(!looped(251, 256));
    assert!(looped(4_864, 256));
}

/// One sample in four at least differs from the one before.
#[test]
fn a_run_that_moves_in_a_quarter_of_its_samples_is_varied() {
    assert!(varied(16, 64));
    assert!(!varied(15, 64));
    assert!(varied(1, 4));
    assert!(!varied(1, 5));
    assert!(!varied(0, 1));
    assert!(varied(4_863, 4_864));
}

#[test]
fn the_lags_are_one_two_and_four_blocks() {
    assert_eq!(lags(64), [64, 128, 256]);
    assert_eq!(lags(128), [128, 256, 512]);
    assert_eq!(lags(480), [480, 960, 1_920]);
}

#[test]
fn ms_is_to_a_tenth() {
    assert_eq!(ms(5_120, 96_000), 53.3);
    assert_eq!(ms(256, 48_000), 5.3);
    // A rate of 0 counts as 1.
    assert_eq!(ms(96, 0), 96_000.0);
}
