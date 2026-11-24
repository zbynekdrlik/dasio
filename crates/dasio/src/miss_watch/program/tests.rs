use super::*;
use crate::miss_watch::{Config, FILE, Setup, config, program_names};
use dstack_audio_io::Block;

const RATE: u32 = 96_000;
const N: usize = 20_000;
const LSB: f64 = 1.0 / 8_388_608.0;

fn cfg(channels: usize) -> Config {
    Config {
        name: "master".into(),
        block: 128,
        rx: vec![],
        tone: vec![],
        program: (0..channels).map(|i| (i, format!("prog {i}"))).collect(),
    }
}

/// A 24-bit sine of `hz` and amplitude `amp`.
fn sine(n: usize, hz: f64, amp: f64, phase: f64) -> Vec<f64> {
    let w = std::f64::consts::TAU * hz / f64::from(RATE);
    (0..n)
        .map(|i| (amp * (w * i as f64 + phase).sin() / LSB).round() * LSB)
        .collect()
}

/// `s` with `skip` samples cut out at `at`: the splice a missed host
/// buffer leaves.
fn splice(s: &[f64], at: usize, skip: usize) -> Vec<f64> {
    let mut out = s[..at].to_vec();
    out.extend_from_slice(&s[at + skip..]);
    out
}

/// The same noise-like signal as the zero/repeat tests: +-0.1 (-20 dBFS).
fn noise(n: usize, seed: u64) -> Vec<f64> {
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

/// Runs `chans` (one Vec per program channel, equal lengths) through a
/// watch in pump blocks of `pump`; at pump block `jump.0` the media
/// position jumps by `jump.1` samples. The events and each channel's
/// figures.
fn run_with(
    chans: &[Vec<f64>],
    pump: usize,
    jump: Option<(usize, u64)>,
) -> (Vec<ProgramJump>, Vec<Summary>) {
    let set = Setup::new(cfg(chans.len()));
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
        let block = Block::new(len, &input, &mut out[..len]).at(start);
        w.check_block(&block);
        pos += len;
        start += len as u64;
        k += 1;
    }
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(set.programs.events.drain(&mut tail, &mut got), 0);
    let sums = (0..chans.len()).map(|c| set.programs.take(c)).collect();
    (got, sums)
}

fn run(chans: &[Vec<f64>]) -> (Vec<ProgramJump>, Vec<Summary>) {
    run_with(chans, 256, None)
}

/// (channel, at) of each event, in push order.
fn places(jumps: &[ProgramJump]) -> Vec<(usize, u64)> {
    jumps.iter().map(|j| (j.channel, j.at)).collect()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= b.abs() * 1e-6, "{a} is not {b}");
}

fn jumps_at(ats: &[(usize, u64)]) -> Vec<ProgramJump> {
    ats.iter()
        .map(|&(channel, at)| ProgramJump {
            channel,
            at,
            ratio: 100.0,
            steps: 5000,
        })
        .collect()
}

#[test]
fn a_clean_sine_gives_no_event_and_is_judged_from_the_first_decision() {
    let (jumps, sums) = run(&[sine(N, 1000.0, 0.5, 0.0)]);
    assert_eq!(jumps, vec![]);
    assert_eq!(
        sums[0],
        Summary {
            checked: (N - 530) as u64,
            jumps: 0,
            worst: 0.0,
            esum: 487_932_928,
        }
    );
}

#[test]
fn a_block_skipped_from_a_sine_is_one_event_at_the_splice() {
    let s = splice(&sine(N, 1000.0, 0.5, 0.0), 5000, 128);
    let (jumps, sums) = run(&[s]);
    assert_eq!(places(&jumps), vec![(0, 5002)]);
    close(jumps[0].ratio, 19816.43501584979);
    // The peak of `e`, in 24-bit steps: a Python model of the same integer
    // signal (round(0.5 sin(2 pi 1000 i / 96000) 2^23), 128 samples cut at
    // 5000, h = x - 4 x1 + 6 x2 - 4 x3 + x4) has its largest |h| at 5002.
    assert_eq!(jumps[0].steps, 952_350);
    assert_eq!(sums[0].checked, (N - 128 - 530) as u64);
    assert_eq!(sums[0].jumps, 1);
    close(sums[0].worst, 19816.43501584979);
    assert_eq!(sums[0].esum, 1_458_543_616);
}

#[test]
fn the_splice_lines_carry_the_distance_to_the_previous_one_and_its_remainder() {
    // The second splice's peak is 1280 samples (ten blocks) after the first.
    let base = sine(N, 1000.0, 0.5, 0.0);
    let (jumps, _) = run(&[splice(&splice(&base, 5000, 128), 6281, 128)]);
    assert_eq!(places(&jumps), vec![(0, 5002), (0, 6282)]);
    close(jumps[1].ratio, 383267.92912959383);
    let lines = Tracker::default().lines(&jumps, 128);
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].since_prev, lines[0].rem), (None, None));
    assert_eq!((lines[1].since_prev, lines[1].rem), (Some(1280), Some(0)));
    // 1279 samples later: not a whole number of blocks.
    let (jumps, _) = run(&[splice(&splice(&base, 5000, 128), 6280, 128)]);
    assert_eq!(places(&jumps), vec![(0, 5002), (0, 6281)]);
    let lines = Tracker::default().lines(&jumps, 128);
    assert_eq!((lines[1].since_prev, lines[1].rem), (Some(1279), Some(127)));
}

#[test]
fn a_loud_onset_into_broadband_audio_and_its_end_are_no_event() {
    // Silence, then a full-level 200 Hz sine over -20 dBFS noise: the step
    // is there, but e stays raised after it (and before the end).
    let nz = noise(N, 7);
    let tone = sine(N, 200.0, 0.5, 0.0);
    let onset: Vec<f64> = (0..N)
        .map(|i| {
            if i < 3000 {
                0.0
            } else {
                tone[i - 3000] + nz[i - 3000]
            }
        })
        .collect();
    let (jumps, sums) = run(&[onset]);
    assert_eq!(jumps, vec![]);
    assert!(
        sums[0].worst > 1.0 && sums[0].worst < K,
        "{}",
        sums[0].worst
    );
    let burst: Vec<f64> = (0..N)
        .map(|i| if i < 9000 { tone[i] + nz[i] } else { 0.0 })
        .collect();
    let (jumps, sums) = run(&[burst]);
    assert_eq!(jumps, vec![]);
    assert!(
        sums[0].worst > 1.0 && sums[0].worst < K,
        "{}",
        sums[0].worst
    );
}

#[test]
fn a_bare_sine_starting_from_silence_is_an_event_a_pure_tone_has_no_high_end() {
    // Known limit, kept as a test: after a clean sine's onset the later
    // window is as quiet as the earlier one; real music is not.
    let mut s = vec![0.0; 5000];
    s.extend(sine(N - 5000, 1000.0, 0.5, 0.0));
    let (jumps, sums) = run(&[s]);
    assert_eq!(places(&jumps), vec![(0, 5002)]);
    // Not judged while the channel is digital silence, and its first
    // sample is zero too: 14 999 of the 15 000 after it.
    assert_eq!(sums[0].checked, 14_999);
}

#[test]
fn white_noise_at_minus_20_dbfs_gives_no_event() {
    let (jumps, sums) = run(&[noise(N, 5)]);
    assert_eq!(jumps, vec![]);
    assert_eq!(sums[0].checked, (N - 530) as u64);
    assert!(
        sums[0].worst > 1.0 && sums[0].worst < K,
        "{}",
        sums[0].worst
    );
}

#[test]
fn a_splice_under_the_absolute_floor_gives_no_event() {
    // The same splice, 40 dB lower: peaks of 3 J under 1024 steps.
    let (jumps, sums) = run(&[splice(&sine(N, 1000.0, 1e-5, 0.0), 5000, 128)]);
    assert_eq!(jumps, vec![]);
    assert_eq!(sums[0].worst, 0.0);
    let (jumps, _) = run(&[splice(&sine(N, 1000.0, 1e-3, 0.0), 5000, 128)]);
    assert_eq!(places(&jumps), vec![(0, 5002)]);
    close(jumps[0].ratio, 1068.9122807017543);
}

#[test]
fn two_channels_spliced_at_one_sample_are_one_line_naming_both() {
    let a = splice(&sine(N, 1000.0, 0.5, 0.0), 5000, 128);
    let b = splice(&sine(N, 1300.0, 0.3, 0.4), 5000, 128);
    let (jumps, _) = run(&[a, b]);
    assert_eq!(places(&jumps), vec![(0, 5002), (1, 5002)]);
    let lines = Tracker::default().lines(&jumps, 128);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].at, &lines[0].channels[..]), (5002, &[0, 1][..]));
    close(lines[0].ratio, 91814.59164884218);
}

#[test]
fn peaks_one_sample_apart_on_two_channels_are_still_one_line() {
    let a = splice(&sine(N, 1000.0, 0.5, 0.0), 5000, 128);
    let b = splice(&sine(N, 1200.0, 0.3, 0.4), 5000, 128);
    let (jumps, _) = run(&[a, b]);
    assert_eq!(places(&jumps), vec![(0, 5002), (1, 5001)]);
    let lines = Tracker::default().lines(&jumps, 128);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].at, &lines[0].channels[..]), (5001, &[0, 1][..]));
}

#[test]
fn one_splice_is_one_event_two_a_hold_off_apart_are_two() {
    let base = sine(N, 1000.0, 0.5, 0.0);
    let twice = |second: usize| run(&[splice(&splice(&base, 5000, 128), second, 128)]).0;
    // 110 samples apart: the second is held off.
    assert_eq!(places(&twice(5110)), vec![(0, 5002)]);
    // 600 apart.
    assert_eq!(places(&twice(5600)), vec![(0, 5002), (0, 5601)]);
    // 511 samples apart is held, 512 is not.
    assert_eq!(places(&twice(5512)), vec![(0, 5002)]);
    assert_eq!(places(&twice(5513)), vec![(0, 5002), (0, 5514)]);
}

#[test]
fn digital_silence_is_not_judged_and_a_stop_ends_it_after_its_span() {
    let (jumps, sums) = run(&[vec![0.0; N]]);
    assert_eq!((jumps, sums[0].checked, sums[0].worst), (vec![], 0, 0.0));
    // A sine cut to digital silence at 3000: a step (an event), judged
    // until 531 zero samples have passed.
    let mut s = sine(3000, 1000.0, 0.5, 0.0);
    s.resize(N, 0.0);
    let (jumps, sums) = run(&[s]);
    assert_eq!(places(&jumps), vec![(0, 3001)]);
    assert_eq!(sums[0].checked, 3000);
}

#[test]
fn the_figures_do_not_depend_on_the_pump_block() {
    let s = splice(&sine(N, 1000.0, 0.5, 0.0), 5000, 128);
    let a = run_with(std::slice::from_ref(&s), 256, None);
    for pump in [1, 37, 128, 4096] {
        let b = run_with(std::slice::from_ref(&s), pump, None);
        assert_eq!(places(&a.0), places(&b.0), "pump {pump}");
        assert_eq!(a.1, b.1, "pump {pump}");
    }
}

#[test]
fn a_jump_in_the_media_position_starts_the_channel_again() {
    let s = splice(&sine(N, 1000.0, 0.5, 0.0), 5000, 128);
    // 2560 samples in (pump block 10), the position jumps by 100 000: the
    // splice, 2442 samples into the new stretch, is found at its media
    // position.
    let (jumps, sums) = run_with(std::slice::from_ref(&s), 256, Some((10, 100_000)));
    assert_eq!(places(&jumps), vec![(0, 102_560 + 2442)]);
    // 2030 samples judged before the jump (decisions from sample 530 to
    // 2559), 16 782 after it.
    assert_eq!(sums[0].checked, 2030 + 16_782);
    // A jump 136 samples before the splice leaves it no history.
    let (jumps, _) = run_with(&[s], 256, Some((19, 100_000)));
    assert_eq!(jumps, vec![]);
}

#[test]
fn the_two_windows_cover_exactly_the_samples_they_say() {
    // An impulse at sample 0 gives e = 1, 4, 6, 4, 1 at samples 0 to 4 and
    // zeros after. The later window is the newest 256 samples, the earlier
    // one 526 to 271 samples back: each takes the five values in at its
    // near edge and lets them go at its far edge, one sample at a time.
    let mut c = Channel::new(0);
    let out = ProgramEvents::default();
    let (mut later, mut earlier) = (Vec::new(), Vec::new());
    for i in 0..=531u64 {
        c.feed(if i == 0 { 1.0 } else { 0.0 }, i, &out, 0);
        later.push(c.later);
        earlier.push(c.earlier);
    }
    let at = |v: &[f64], from: usize| v[from..from + 6].to_vec();
    assert_eq!(at(&later, 0), vec![1.0, 5.0, 11.0, 15.0, 16.0, 16.0]);
    assert_eq!(at(&later, 254), vec![16.0, 16.0, 15.0, 11.0, 5.0, 1.0]);
    assert_eq!(later[260], 0.0);
    assert_eq!(at(&earlier, 270), vec![0.0, 1.0, 5.0, 11.0, 15.0, 16.0]);
    assert_eq!(at(&earlier, 525), vec![16.0, 16.0, 15.0, 11.0, 5.0, 1.0]);
    assert_eq!(earlier[531], 0.0);
}

#[test]
fn the_fourth_difference_passes_a_step_as_1_3_3_1() {
    let mut before = [0.0; 4];
    let mut got = Vec::new();
    for x in [1.0, 1.0, 1.0, 1.0, 1.0] {
        got.push(fourth(x, &before));
        before = [x, before[0], before[1], before[2]];
    }
    assert_eq!(got, vec![1.0, -3.0, 3.0, -1.0, 0.0]);
    // A cubic has none; x^4 has 24.
    let p = |n: f64| n.powi(4);
    assert_eq!(fourth(p(4.0), &[p(3.0), p(2.0), p(1.0), p(0.0)]), 24.0);
    let c = |n: f64| n.powi(3);
    assert_eq!(fourth(c(4.0), &[c(3.0), c(2.0), c(1.0), c(0.0)]), 0.0);
}

#[test]
fn a_jump_is_above_the_floor_and_k_times_each_mean() {
    assert!(jump(FLOOR * 2.0, 0.0, 0.0));
    assert!(!jump(FLOOR, 0.0, 0.0));
    assert!(above_floor(FLOOR + 1e-12) && !above_floor(FLOOR));
    // At exactly K times a mean it counts; under it it does not, for each.
    assert!(jump(1.0, 1.0 / K, 0.0));
    assert!(!jump(1.0, 1.0 / K + 1e-9, 0.0));
    assert!(jump(1.0, 0.0, 1.0 / K));
    assert!(!jump(1.0, 0.0, 1.0 / K + 1e-9));
    assert!(!jump(1.0, 1.0, 1.0));
    assert_eq!(FLOOR, 1024.0 * LSB);
}

#[test]
fn the_ratio_is_over_the_larger_mean_and_at_least_one_step() {
    assert_eq!(ratio(8.0, 2.0, 1.0), 4.0);
    assert_eq!(ratio(8.0, 1.0, 2.0), 4.0);
    assert_eq!(ratio(1.0, 0.0, 0.0), 1.0 / QUANT);
    assert_eq!(ratio(1.0, QUANT / 2.0, 0.0), 1.0 / QUANT);
}

#[test]
fn a_window_mean_is_its_sum_over_the_window_and_never_negative() {
    assert_eq!(mean(512.0), 2.0);
    assert_eq!(mean(-1e-12), 0.0);
}

#[test]
fn a_peak_is_held_off_for_a_hold_after_an_event() {
    assert!(!held(1_000, None));
    assert!(held(1_000 + HOLD - 1, Some(1_000)));
    assert!(!held(1_000 + HOLD, Some(1_000)));
    assert!(!held(1_000 + HOLD + 1, Some(1_000)));
}

#[test]
fn a_sample_is_judged_until_a_whole_decision_span_of_zeros() {
    assert!(judged(0));
    assert!(judged(530));
    assert!(!judged(531));
}

#[test]
fn a_local_peak_is_the_first_of_equal_values_within_four_either_way() {
    let ring = |set: &[(usize, f64)]| {
        let mut e = vec![0.0; RING];
        for &(i, v) in set {
            e[i] = v;
        }
        e
    };
    let at = 100;
    assert!(local_peak(&ring(&[(at, 1.0)]), at as u64));
    // Larger within four, either side: not a peak.
    for k in 1..=NEAR {
        assert!(
            !local_peak(&ring(&[(at, 1.0), (at - k, 1.5)]), at as u64),
            "{k}"
        );
        assert!(
            !local_peak(&ring(&[(at, 1.0), (at + k, 1.5)]), at as u64),
            "{k}"
        );
    }
    // Larger at five away does not matter.
    assert!(local_peak(
        &ring(&[(at, 1.0), (at - 5, 1.5), (at + 5, 1.5)]),
        at as u64
    ));
    // Equal: before it, not a peak; after it, a peak (the first of two).
    assert!(!local_peak(&ring(&[(at, 1.0), (at - 1, 1.0)]), at as u64));
    assert!(local_peak(&ring(&[(at, 1.0), (at + 1, 1.0)]), at as u64));
    assert!(!local_peak(&ring(&[(at, 1.0), (at - 4, 1.0)]), at as u64));
    assert!(local_peak(&ring(&[(at, 1.0), (at + 4, 1.0)]), at as u64));
    // Around the ring's end.
    assert!(local_peak(
        &ring(&[(RING - 1, 1.0), (0, 0.5), (RING - 3, 0.5)]),
        RING as u64 - 1
    ));
    assert!(!local_peak(
        &ring(&[(1, 1.0), (RING - 1, 1.5)]),
        RING as u64 + 1
    ));
    assert!(!local_peak(
        &ring(&[(RING - 2, 1.0), (1, 1.5)]),
        RING as u64 - 2
    ));
}

#[test]
fn the_floor_level_is_the_mean_of_e_in_dbfs() {
    let one = |esum: u64, checked: u64| Summary {
        checked,
        jumps: 0,
        worst: 0.0,
        esum,
    };
    // A mean of 2^-10 over 4 samples: -60.2 dB.
    let e = (FIXED / 1024.0) as u64;
    close(floor_dbfs(&one(4 * e, 4)), -60.205_999_132_796_24);
    // Nothing: -240, and no division by zero with nothing checked.
    close(floor_dbfs(&one(0, 0)), -240.0);
}

#[test]
fn an_event_round_trips_through_a_slot() {
    let j = ProgramJump {
        channel: 13,
        at: 0x1234_5678_9abc,
        ratio: 1234.5678,
        steps: 0x7fff_ffff_ffff,
    };
    assert_eq!(ProgramJump::unpack(j.pack()), j);
    let events = ProgramEvents::default();
    events.push(j);
    let (mut tail, mut got) = (0, Vec::new());
    assert_eq!(events.drain(&mut tail, &mut got), 0);
    assert_eq!(got, vec![j]);
}

#[test]
fn the_stats_take_empties_and_keep_the_worst() {
    let s = Stats::default();
    let mut c = Channel::new(0);
    c.checked = 5;
    c.jumps = 1;
    c.worst = 3.5;
    c.esum = 99;
    c.publish(&s);
    assert_eq!((c.checked, c.jumps, c.worst, c.esum), (0, 0, 0.0, 0));
    c.checked = 2;
    c.worst = 2.0;
    c.publish(&s);
    assert_eq!(
        s.take(),
        Summary {
            checked: 7,
            jumps: 1,
            worst: 3.5,
            esum: 99
        }
    );
    assert_eq!(s.take().checked, 0);
}

#[test]
fn programs_keep_one_set_of_figures_per_channel() {
    let p = Programs::new(2);
    let mut c = Channel::new(0);
    c.checked = 4;
    c.publish(p.stats(1));
    assert_eq!((p.take(0).checked, p.take(1).checked), (0, 4));
}

#[test]
fn lines_group_events_within_one_sample_of_the_first_and_no_further() {
    let mut t = Tracker::default();
    // 1000, 1001 are one line; 1002 is 2 from the first: its own.
    let lines = t.lines(&jumps_at(&[(2, 1001), (0, 1000), (1, 1002)]), 128);
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].at, &lines[0].channels[..]), (1000, &[0, 2][..]));
    assert_eq!((lines[1].at, &lines[1].channels[..]), (1002, &[1][..]));
    // The same channel twice in one line is named once; the line keeps the
    // largest ratio.
    let mut j = jumps_at(&[(0, 5000), (0, 5001)]);
    j[1].ratio = 250.0;
    let lines = Tracker::default().lines(&j, 128);
    assert_eq!(lines.len(), 1);
    assert_eq!((&lines[0].channels[..], lines[0].ratio), (&[0][..], 250.0));
}

#[test]
fn the_distance_to_the_previous_event_is_per_channel_and_survives_ticks() {
    let mut t = Tracker::default();
    let l = t.lines(&jumps_at(&[(0, 1000), (1, 1000)]), 100);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].since_prev, None);
    // Next tick: only channel 1 again, 750 later; channel 0 is not asked.
    let l = t.lines(&jumps_at(&[(1, 1750)]), 100);
    assert_eq!((l[0].since_prev, l[0].rem), (Some(750), Some(50)));
    // Channel 0 comes back: its own previous event is 1000, not 1750.
    let l = t.lines(&jumps_at(&[(0, 2000)]), 100);
    assert_eq!((l[0].since_prev, l[0].rem), (Some(1000), Some(0)));
    // A line takes the distance of its first (earliest) event's channel.
    let l = t.lines(&jumps_at(&[(0, 3001), (1, 3000)]), 100);
    assert_eq!(
        (&l[0].channels[..], l[0].since_prev),
        (&[0, 1][..], Some(1250))
    );
}

/// The device's RX channel names the config tests read against.
fn names() -> Vec<String> {
    ["Hand 1 SNV", "CG SNV L", "CG SNV R", "Ruchy 1"]
        .map(String::from)
        .to_vec()
}

#[test]
fn the_config_reads_program_channels_next_to_the_other_lists() {
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\nprogram = [\"CG SNV R\", \"CG SNV L\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!(c.rx, vec![(0, "Hand 1 SNV".to_string())]);
    assert_eq!(c.tone, vec![]);
    assert_eq!(
        c.program,
        vec![(2, "CG SNV R".to_string()), (1, "CG SNV L".to_string())]
    );
    // A channel may be in all three lists.
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Ruchy 1\"]\ntone = [\"Ruchy 1\"]\nprogram = [\"Ruchy 1\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!((c.rx.len(), c.tone.len(), c.program.len()), (1, 1, 1));
    // A file without the key still reads.
    let c = config(
        "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!(c.program, vec![]);
}

#[test]
fn a_program_only_config_needs_no_rx_and_no_other_list() {
    for text in [
        "name = \"m\"\nblock = 128\nprogram = [\"Ruchy 1\"]\n",
        "name = \"m\"\nblock = 128\nrx = []\nprogram = [\"Ruchy 1\"]\n",
    ] {
        let c = config(text, &names()).unwrap();
        assert_eq!(
            (c.rx, c.tone, c.program),
            (vec![], vec![], vec![(3, "Ruchy 1".to_string())]),
            "{text}"
        );
    }
}

#[test]
fn the_config_refuses_a_program_list_it_cannot_watch() {
    let rx = "name = \"m\"\nblock = 128\nrx = [\"Hand 1 SNV\"]\n";
    for (program, e) in [
        ("program = []", "`program` must name 1 to 16 channels"),
        (
            "program = \"Ruchy 1\"",
            "`program` must be a list of RX channel names",
        ),
        ("program = [2]", "`program` entries must be strings"),
        (
            "program = [\"PETA\"]",
            "`program` names \"PETA\", which is no RX channel of this device",
        ),
        (
            "program = [\"Ruchy 1\", \"Ruchy 1\"]",
            "`program` names \"Ruchy 1\" twice",
        ),
    ] {
        for text in [
            format!("{rx}{program}\n"),
            format!("name = \"m\"\nblock = 128\n{program}\n"),
        ] {
            assert_eq!(
                config(&text, &names()).unwrap_err(),
                format!("{FILE}: {e}"),
                "{text}"
            );
        }
    }
}

#[test]
fn the_program_list_takes_sixteen_channels_and_no_more() {
    let n17: Vec<String> = (0..17).map(|i| format!("c{i}")).collect();
    let list = |k: usize| {
        n17[..k]
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let c = config(
        &format!("name = \"m\"\nblock = 128\nprogram = [{}]\n", list(16)),
        &n17,
    )
    .unwrap();
    assert_eq!((c.program.len(), c.program[15].0), (16, 15));
    assert_eq!(
        config(
            &format!("name = \"m\"\nblock = 128\nprogram = [{}]\n", list(17)),
            &n17
        )
        .unwrap_err(),
        format!("{FILE}: `program` must name 1 to 16 channels")
    );
}

#[test]
fn program_names_are_the_config_s_words_in_the_order_given() {
    let c = config(
        "name = \"m\"\nblock = 128\nprogram = [\"CG SNV R\", \"Hand 1 SNV\", \"CG SNV L\"]\n",
        &names(),
    )
    .unwrap();
    assert_eq!(program_names(&[0, 2], &c), "CG SNV R, CG SNV L");
    assert_eq!(program_names(&[1], &c), "Hand 1 SNV");
    assert_eq!(program_names(&[7], &c), "");
}

#[test]
fn a_setup_starts_its_program_figures_one_per_program_channel() {
    let c = config(
        "name = \"m\"\nblock = 128\nprogram = [\"CG SNV L\", \"CG SNV R\"]\n",
        &names(),
    )
    .unwrap();
    let set = Setup::new(c);
    assert_eq!(set.programs.take(1).checked, 0);
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(set.programs.events.drain(&mut tail, &mut got), 0);
    assert!(got.is_empty());
}

#[test]
fn a_peak_is_carried_in_steps_and_read_in_dbfs_to_a_tenth() {
    assert_eq!(steps(1024.0 * QUANT), 1024);
    assert_eq!(steps(952_350.0 * QUANT), 952_350);
    assert_eq!(peak_dbfs(8_388_608), 0.0);
    assert_eq!(peak_dbfs(4_194_304), -6.0);
    assert_eq!(peak_dbfs(1024), -78.3);
    // 3 steps: -128.9 dBFS; none counts as one step, never -inf.
    assert_eq!(peak_dbfs(0), peak_dbfs(1));
    assert!(peak_dbfs(0).is_finite());
}

#[test]
fn the_grid_is_the_place_in_the_block_and_cannot_overflow() {
    assert_eq!(grid(0, 128), 0);
    assert_eq!(grid(127, 128), 127);
    assert_eq!(grid(128, 128), 0);
    assert_eq!(grid(3 * 128 + 5, 128), 5);
    assert_eq!(grid(u64::MAX, 128), 127);
    assert_eq!(grid(u64::MAX, 64), 63);
}

#[test]
fn a_line_carries_the_largest_peak_and_the_first_events_grid() {
    let mut j = jumps_at(&[(0, 1000), (1, 1001)]);
    j[0].steps = 3000;
    j[1].steps = 9000;
    let l = Tracker::default().lines(&j, 128);
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].steps, l[0].grid), (9000, 1000 % 128));
}
