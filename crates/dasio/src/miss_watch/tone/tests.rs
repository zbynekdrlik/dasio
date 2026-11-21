use dstack_audio_io::{Block, Process};

use super::*;
use crate::miss_watch::{Config, Setup, Watch, Watched, config};

const RATE: u32 = 96_000;
/// The first residual checked after a clean first fit: two samples of
/// history, then [`ACQUIRE`] residuals.
const SETTLED_AT: u64 = 2 + ACQUIRE as u64;
/// One 24-bit step.
const STEP: f64 = 1.0 / 8_388_608.0;

/// A sine of `hz` at `dbfs`, quantised to 24 bits as Dante carries it: `n`
/// samples at `rate`.
fn tone_at(n: usize, hz: f64, dbfs: f64, rate: u32) -> Vec<f64> {
    let a = 10f64.powf(dbfs / 20.0);
    let w = TAU * hz / f64::from(rate);
    (0..n)
        .map(|i| (a * (w * i as f64 + 0.3).sin() / STEP).round() * STEP)
        .collect()
}

fn tone(n: usize, hz: f64, dbfs: f64) -> Vec<f64> {
    tone_at(n, hz, dbfs, RATE)
}

/// The same sine with its level falling `db_per_s` from `dbfs`.
fn fading(n: usize, dbfs: f64, db_per_s: f64) -> Vec<f64> {
    let w = TAU * 1_000.0 / f64::from(RATE);
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let a = 10f64.powf((dbfs - db_per_s * t) / 20.0);
            (a * (w * i as f64 + 0.3).sin() / STEP).round() * STEP
        })
        .collect()
}

/// A watch of `channels` tone channels, RX 0 and on.
fn cfg(channels: usize) -> Config {
    Config {
        name: "master".into(),
        block: 128,
        rx: vec![],
        tone: (0..channels).map(|i| (i, format!("free {i}"))).collect(),
    }
}

/// Feeds samples `range` of `chans` (the device's RX channels, one Vec
/// each) to `w` in pump blocks of `block`, each at its own position.
fn feed(w: &mut Watch, chans: &[Vec<f64>], range: std::ops::Range<usize>, block: usize) {
    let mut out = vec![0.0; block];
    let mut input = Vec::with_capacity(block * chans.len());
    for pos in range.clone().step_by(block) {
        let len = block.min(range.end - pos);
        input.clear();
        for c in chans {
            input.extend_from_slice(&c[pos..pos + len]);
        }
        w.check_block(&Block::new(len, &input, &mut out[..len]).at(pos as u64));
    }
}

/// `chans` (one per tone channel) through a watch at `rate` in pump blocks
/// of `block`: the tone breaks and each channel's figures.
fn run_at(chans: &[Vec<f64>], block: usize, rate: u32) -> (Vec<ToneBreak>, Vec<Summary>) {
    let set = Setup::new(cfg(chans.len()));
    let mut w = set.watch(rate);
    feed(&mut w, chans, 0..chans[0].len(), block);
    let mut tail = 0;
    let mut got = Vec::new();
    assert_eq!(set.tones.events.drain(&mut tail, &mut got), 0);
    let figures = (0..chans.len()).map(|i| set.tones.take(i)).collect();
    (got, figures)
}

/// One tone channel at the device rate in blocks of 128.
fn run(s: &[f64]) -> (Vec<ToneBreak>, Summary) {
    let (breaks, figures) = run_at(&[s.to_vec()], 128, RATE);
    (breaks, figures[0])
}

/// The breaks as (kind, at, len).
fn shape(breaks: &[ToneBreak]) -> Vec<(ToneKind, u64, u32)> {
    breaks.iter().map(|b| (b.kind, b.at, b.len)).collect()
}

#[test]
fn clean_tones_have_no_break_and_are_checked_from_the_first_fit() {
    for hz in [50.0, 440.0, 1_000.0, 5_000.0, 15_000.0] {
        for dbfs in [-60.0, -40.0, -20.0, -6.0] {
            let s = tone(48_000, hz, dbfs);
            for block in [64, 128, 256] {
                let (breaks, figures) = run_at(&[s.clone()], block, RATE);
                let f = figures[0];
                assert_eq!(
                    (breaks, f.checked, f.breaks),
                    (vec![], 48_000 - SETTLED_AT, 0),
                    "{hz} Hz at {dbfs} dBFS, blocks of {block}"
                );
            }
        }
    }
}

#[test]
fn quiet_tones_down_to_the_gate_have_no_break() {
    // 42 Hz at -79 dBFS: a window of 256 samples (not the tone's period)
    // would read its level 0.2 dB off.
    for (hz, dbfs) in [
        (42.0, -79.0),
        (50.0, -79.0),
        (50.0, -72.0),
        (1_000.0, -79.0),
        (1_000.0, -66.0),
    ] {
        let (breaks, f) = run(&tone(96_000, hz, dbfs));
        assert_eq!(
            (breaks, f.checked),
            (vec![], 96_000 - SETTLED_AT),
            "{hz} Hz at {dbfs} dBFS"
        );
    }
}

#[test]
fn no_tone_is_not_checked() {
    let n = 20_000;
    let mut s = 7u64;
    let noise: Vec<f64> = (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.2
        })
        .collect();
    // The tone with noise 40 dB under it: no clean fit.
    let noisy: Vec<f64> = tone(n, 1_000.0, -20.0)
        .iter()
        .zip(&noise)
        .map(|(t, z)| t + z * 0.1)
        .collect();
    for (what, s) in [
        ("silence", vec![0.0; n]),
        ("-81 dBFS", tone(n, 1_000.0, -81.0)),
        ("-90 dBFS", tone(n, 1_000.0, -90.0)),
        ("noise", noise),
        ("a tone in noise", noisy),
        ("30 Hz", tone(n, 30.0, -20.0)),
        ("36 Hz", tone(n, 36.0, -20.0)),
        ("43.8 kHz", tone(n, 43_800.0, -20.0)),
    ] {
        let (breaks, f) = run(&s);
        assert_eq!((breaks, f.checked, f.breaks), (vec![], 0, 0), "{what}");
    }
}

#[test]
fn tones_just_inside_the_band_are_checked() {
    for hz in [45.0, 42_000.0] {
        let (breaks, f) = run(&tone(20_000, hz, -20.0));
        assert_eq!(
            (breaks, f.checked),
            (vec![], 20_000 - SETTLED_AT),
            "{hz} Hz"
        );
    }
}

#[test]
fn a_one_sample_click_at_minus_60_db_is_one_break() {
    let mut s = tone(48_000, 1_000.0, -20.0);
    s[30_000] += 0.1 * REL;
    let (breaks, f) = run(&s);
    assert_eq!(shape(&breaks), vec![(ToneKind::Click, 30_000, 1)]);
    assert_eq!(breaks[0].channel, 0);
    // The residual next to a click of d at 1 kHz is 2 cos w x d: -54 dB.
    let rel = db(breaks[0].rel);
    assert!((-54.5..=-53.5).contains(&rel), "{rel}");
    assert_eq!(f.breaks, 1);
}

#[test]
fn a_dropped_sample_is_one_break() {
    let s = tone(48_001, 1_000.0, -20.0);
    let mut d = s[..30_000].to_vec();
    d.extend_from_slice(&s[30_001..]);
    let (breaks, _) = run(&d);
    assert_eq!(shape(&breaks), vec![(ToneKind::Click, 30_000, 1)]);
    // A slip of one sample at 1 kHz: the residual is about A w, -24 dB.
    let rel = db(breaks[0].rel);
    assert!((-25.0..=-23.0).contains(&rel), "{rel}");
}

#[test]
fn a_zero_run_is_one_break_of_its_length() {
    for len in [1, 7, 64, 300] {
        let mut s = tone(48_000, 1_000.0, -20.0);
        s[30_000..30_000 + len].fill(0.0);
        let (breaks, _) = run(&s);
        assert_eq!(
            shape(&breaks),
            vec![(ToneKind::Click, 30_000, len as u32)],
            "a zero run of {len}"
        );
    }
}

#[test]
fn a_repeated_block_is_one_break() {
    // The block before played again: a phase jump at its start (one
    // sample) and another at its end, within the next fit.
    let mut s = tone(48_000, 1_000.0, -20.0);
    s.copy_within(30_000..30_128, 30_128);
    let (breaks, _) = run(&s);
    assert_eq!(shape(&breaks), vec![(ToneKind::Click, 30_128, 1)]);
}

#[test]
fn a_break_across_a_block_boundary_is_one_break() {
    // 29 952 is a boundary of blocks of 64, 128 and 256.
    let at = 29_952 - 20;
    for block in [64, 128, 256] {
        let mut s = tone(48_000, 1_000.0, -20.0);
        s[at..at + 64].fill(0.0);
        let (breaks, _) = run_at(&[s], block, RATE);
        assert_eq!(
            shape(&breaks),
            vec![(ToneKind::Click, at as u64, 64)],
            "blocks of {block}"
        );
    }
}

#[test]
fn damage_resumed_within_the_clean_run_is_one_break() {
    // Two clicks of -40 dB: the first one's residuals end 3 samples on, so
    // a second within RESUME + 2 samples is the same break; one sample
    // later the first has ended and the second falls in the next fit.
    for (gap, len) in [(8, 9), (RESUME + 2, RESUME as u32 + 3), (RESUME + 3, 1)] {
        let mut s = tone(48_000, 1_000.0, -20.0);
        s[30_000] += 1e-3;
        s[30_000 + gap] += 1e-3;
        let (breaks, _) = run(&s);
        assert_eq!(
            shape(&breaks),
            vec![(ToneKind::Click, 30_000, len)],
            "clicks {gap} apart"
        );
    }
}

#[test]
fn a_small_level_step_is_one_level_break() {
    // A 0.2 dB rise over 64 samples from the start of the fit's 41st level
    // window (1 kHz: windows of LEVEL_MIN): too smooth for a click, a
    // level break of that window.
    let from = SETTLED_AT as usize + 40 * LEVEL_MIN;
    let gain = 10f64.powf(0.2 / 20.0);
    let mut s = tone(48_000, 1_000.0, -20.0);
    for (i, x) in s.iter_mut().enumerate().skip(from) {
        let part = ((i - from) as f64 / 64.0).min(1.0);
        *x = (*x * (1.0 + (gain - 1.0) * part) / STEP).round() * STEP;
    }
    let (breaks, f) = run(&s);
    assert_eq!(
        shape(&breaks),
        vec![(ToneKind::Level, from as u64, LEVEL_MIN as u32)]
    );
    let step = db(breaks[0].rel);
    assert!((0.15..=0.2).contains(&step), "{step}");
    assert_eq!(f.breaks, 1);
}

#[test]
fn a_slow_fade_moves_the_running_level_without_a_break() {
    // 0.14 dB/s: the running amplitude trails by about 0.14 x RUN_S =
    // 0.07 dB, inside the 0.1 dB step (it would not with a 1 s constant).
    let n = 3 * RATE as usize;
    let (breaks, f) = run(&fading(n, -20.0, 0.14));
    assert_eq!((breaks, f.checked), (vec![], n as u64 - SETTLED_AT));
}

#[test]
fn a_tone_faded_to_the_gate_is_no_longer_checked() {
    // From -79.8 dBFS at 0.1 dB/s the running amplitude reaches -80 dBFS
    // at about 2.5 s: from there the channel fits again, quietly, and
    // never settles.
    let n = 4 * RATE as usize;
    let s = [fading(n, -79.8, 0.1)];
    let set = Setup::new(cfg(1));
    let mut w = set.watch(RATE);
    let three = 3 * RATE as usize;
    feed(&mut w, &s, 0..three, 128);
    assert!(set.tones.take(0).checked > 2 * u64::from(RATE));
    feed(&mut w, &s, three..n, 128);
    let last = set.tones.take(0);
    assert_eq!((last.checked, last.breaks), (0, 0));
    let mut tail = 0;
    let mut got = Vec::new();
    set.tones.events.drain(&mut tail, &mut got);
    assert_eq!(got, vec![]);
}

#[test]
fn a_tone_that_stops_is_one_break_of_a_second() {
    // At 8 kHz a second is 8 000 samples: the break started at the first
    // zero ends with the sample that makes it a second long.
    let rate = 8_000;
    let mut s = tone_at(3 * 8_000, 1_000.0, -20.0, rate);
    s[5_000..].fill(0.0);
    let set = Setup::new(cfg(1));
    let mut w = set.watch(rate);
    let mut out = [0.0];
    let mut tail = 0;
    let mut got = Vec::new();
    for (i, x) in s.iter().enumerate() {
        w.check_block(&Block::new(1, std::slice::from_ref(x), &mut out).at(i as u64));
        set.tones.events.drain(&mut tail, &mut got);
        if !got.is_empty() {
            assert_eq!(i, 5_000 + 8_000 - 1);
            break;
        }
    }
    assert_eq!(shape(&got), vec![(ToneKind::Click, 5_000, 8_000)]);
}

#[test]
fn a_tone_after_silence_is_found_by_the_first_clean_fit() {
    // The fits over silence fail, the one over the tone's start fails, the
    // next (residuals 10 242 to 12 289) settles.
    let mut s = tone(30_000, 1_000.0, -20.0);
    s[..10_000].fill(0.0);
    let (breaks, f) = run(&s);
    assert_eq!((breaks, f.checked), (vec![], 30_000 - 12_290));
}

#[test]
fn a_jump_in_the_media_position_starts_the_tone_again() {
    let s = [tone(40_000, 1_000.0, -20.0)];
    let set = Setup::new(cfg(1));
    let mut w = set.watch(RATE);
    feed(&mut w, &s, 0..20_000, 128);
    // 10 000 samples later, the tone's phase with them: a new fit.
    feed(&mut w, &s, 30_000..40_000, 128);
    let mut tail = 0;
    let mut got = Vec::new();
    set.tones.events.drain(&mut tail, &mut got);
    assert_eq!(got, vec![]);
    assert_eq!(
        set.tones.take(0).checked,
        (20_000 - SETTLED_AT) + (10_000 - SETTLED_AT)
    );
}

struct Nothing;
impl Process for Nothing {
    fn process(&mut self, _: &mut Block<'_>) {}
}

#[test]
fn a_discontinuity_starts_the_tone_again() {
    // The positions run on, the tone jumps: after a discontinuity that is
    // a new fit, not a break.
    let s = tone(40_000, 1_000.0, -20.0);
    let set = Setup::new(cfg(1));
    let mut p = Watched::new(Nothing, Some(set.watch(RATE)));
    let mut out = [0.0; 100];
    for pos in (0..20_000).step_by(100) {
        p.process(&mut Block::new(100, &s[pos..pos + 100], &mut out).at(pos as u64));
    }
    p.discontinuity();
    for pos in (20_000..30_000).step_by(100) {
        let from = pos + 10_000;
        p.process(&mut Block::new(100, &s[from..from + 100], &mut out).at(pos as u64));
    }
    let mut tail = 0;
    let mut got = Vec::new();
    set.tones.events.drain(&mut tail, &mut got);
    assert_eq!(got, vec![]);
}

#[test]
fn the_figures_count_checks_breaks_and_read_the_tone() {
    let mut s = tone(RATE as usize, 1_000.0, -20.0);
    s[30_000] += 0.1 * REL;
    let set = Setup::new(cfg(1));
    let mut w = set.watch(RATE);
    feed(&mut w, &[s], 0..RATE as usize, 128);
    let f = set.tones.take(0);
    // Checked from the first fit on, less the fit after the click.
    assert_eq!(f.checked, u64::from(RATE) - SETTLED_AT - ACQUIRE as u64);
    assert_eq!(f.breaks, 1);
    let worst = db(f.worst);
    assert!((-54.5..=-53.5).contains(&worst), "{worst}");
    for a in [f.amp_min, f.amp_max] {
        assert!((-20.01..=-19.99).contains(&db(a)), "{}", db(a));
    }
    let hz = hz(f.c, RATE);
    assert!((hz - 1_000.0).abs() < 0.05, "{hz}");
    // The next stretch starts empty; the fit stays.
    let next = set.tones.take(0);
    assert_eq!(
        (
            next.checked,
            next.breaks,
            next.worst,
            next.amp_min,
            next.amp_max
        ),
        (0, 0, 0.0, f64::INFINITY, 0.0)
    );
    assert_eq!(next.c.to_bits(), f.c.to_bits());
}

#[test]
fn a_channel_never_fitted_has_no_tone() {
    let f = Tones::new(1).take(0);
    assert_eq!(
        (f.checked, f.breaks, f.worst, f.amp_min, f.amp_max),
        (0, 0, 0.0, f64::INFINITY, 0.0)
    );
    assert!(f.c.is_nan());
}

#[test]
fn a_setup_s_watch_reads_each_tone_channel_s_rx_into_its_own_figures() {
    let names = ["a", "b", "free 01", "free 02"].map(String::from);
    let c = config(
        "name = \"m\"\nblock = 128\ntone = [\"free 02\", \"free 01\"]\n",
        &names,
    )
    .unwrap();
    let set = Setup::new(c);
    let mut w = set.watch(RATE);
    // RX 2 ("free 01", the tone list's second) has a gap of 7; RX 3 is
    // clean; RX 0 and 1 hold a constant.
    let clean = tone(20_000, 1_000.0, -20.0);
    let mut gap = tone(20_000, 440.0, -20.0);
    gap[10_000..10_007].fill(0.0);
    let flat = vec![0.3; 20_000];
    feed(&mut w, &[flat.clone(), flat, gap, clean], 0..20_000, 128);
    let mut tail = 0;
    let mut got = Vec::new();
    set.tones.events.drain(&mut tail, &mut got);
    assert_eq!(shape(&got), vec![(ToneKind::Click, 10_000, 7)]);
    assert_eq!(got[0].channel, 1);
    let (free02, free01) = (set.tones.take(0), set.tones.take(1));
    assert_eq!((free02.checked, free02.breaks), (20_000 - SETTLED_AT, 0));
    assert_eq!(
        (free01.checked, free01.breaks),
        (20_000 - SETTLED_AT - ACQUIRE as u64, 1)
    );
}

#[test]
fn a_tone_break_round_trips_through_a_slot() {
    for b in [
        ToneBreak {
            channel: 0,
            kind: ToneKind::Click,
            at: 0,
            len: 0,
            rel: 0.0,
        },
        ToneBreak {
            channel: 15,
            kind: ToneKind::Level,
            at: u64::MAX,
            len: u32::MAX,
            rel: 1.5,
        },
        ToneBreak {
            channel: 7,
            kind: ToneKind::Click,
            at: 123_456_789,
            len: 1_024,
            rel: 2e-3,
        },
    ] {
        let events = ToneEvents::default();
        events.push(b);
        let mut tail = 0;
        let mut got = Vec::new();
        assert_eq!(events.drain(&mut tail, &mut got), 0);
        assert_eq!(got, vec![b]);
    }
}

#[test]
fn tone_kind_names_are_the_log_s_words() {
    assert_eq!(ToneKind::Click.name(), "click");
    assert_eq!(ToneKind::Level.name(), "level");
}

#[test]
fn above_is_strictly_beyond_either_way() {
    assert!(!above(0.5, 0.5));
    assert!(!above(-0.5, 0.5));
    assert!(above(0.5f64.next_up(), 0.5));
    assert!(above(-0.5f64.next_up(), 0.5));
    assert!(!above(0.25, 0.5));
}

#[test]
fn the_band_is_open_at_both_ends_and_takes_no_nan() {
    let (lo, hi) = (0.5, 1.5);
    assert!(!in_band(lo, lo, hi));
    assert!(!in_band(hi, lo, hi));
    assert!(in_band(lo.next_up(), lo, hi));
    assert!(in_band(hi.next_down(), lo, hi));
    assert!(!in_band(0.0, lo, hi));
    assert!(!in_band(f64::NAN, lo, hi));
}

#[test]
fn present_is_strictly_above_the_gate() {
    assert!(!present(AMP_MIN));
    assert!(present(AMP_MIN.next_up()));
}

#[test]
fn the_threshold_is_minus_60_db_of_the_tone_or_the_quantisation_floor() {
    assert_eq!(threshold(1.0), REL);
    assert_eq!(threshold(0.5), 0.5 * REL);
    assert_eq!(threshold(0.0), QFLOOR);
    assert_eq!(threshold(1e-4), QFLOOR);
}

#[test]
fn a_fit_settles_up_to_an_rms_residual_of_half_the_threshold() {
    // A threshold of 3: settled up to a residual energy of 9 x ACQUIRE / 4.
    let edge = 9.0 * ACQUIRE as f64 / 4.0;
    assert!(settles(edge, 3.0));
    assert!(!settles(edge.next_up(), 3.0));
    assert!(settles(0.0, 3.0));
}

#[test]
fn a_level_break_is_a_step_beyond_a_tenth_of_a_db_either_way() {
    assert!(!level_break(1.0));
    assert!(!level_break(LEVEL_STEP));
    assert!(level_break(LEVEL_STEP.next_up()));
    assert!(!level_break(1.0 / LEVEL_STEP));
    assert!(level_break((1.0 / LEVEL_STEP).next_down()));
    assert!(level_break(f64::NAN));
    assert!((db(LEVEL_STEP) - 0.1).abs() < 1e-12);
}

#[test]
fn the_level_window_is_a_period_rounded_up_at_least_level_min() {
    assert_eq!(window(2.0 * (TAU / 1_000.5).cos()), 1_001);
    assert_eq!(window(2.0 * (TAU / 100.5).cos()), LEVEL_MIN);
}

#[test]
fn db_and_hz_read_a_ratio_and_a_fit() {
    assert_eq!(db(10.0), 20.0);
    assert!((db(REL) + 60.0).abs() < 1e-9);
    let c = |hz: f64, rate: u32| 2.0 * (TAU * hz / f64::from(rate)).cos();
    assert!((hz(c(1_000.0, 96_000), 96_000) - 1_000.0).abs() < 1e-6);
    assert!((hz(c(440.0, 48_000), 48_000) - 440.0).abs() < 1e-6);
}
