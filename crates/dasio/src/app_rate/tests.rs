use super::*;
use dstack_resample::{Converter, Fixed};

#[test]
fn the_app_block_is_the_device_block_s_period_at_the_app_s_rate_rounded_up() {
    let p = Plan::new(96_000, 48_000, 32).unwrap();
    assert_eq!((p.app_block, p.exact, p.converts()), (16, true, true));
    let p = Plan::new(96_000, 44_100, 128).unwrap();
    // 128 * 44100 / 96000 = 58.8
    assert_eq!((p.app_block, p.exact, p.converts()), (59, false, true));
    let p = Plan::new(96_000, 44_100, 16).unwrap();
    // 7.35
    assert_eq!((p.app_block, p.exact), (8, false));
    let p = Plan::new(96_000, 44_100, 256).unwrap();
    // 117.6
    assert_eq!(p.app_block, 118);
    let p = Plan::new(96_000, 96_000, 64).unwrap();
    assert_eq!((p.app_block, p.exact, p.converts()), (64, true, false));
    assert_eq!(
        (p.device_rate, p.app_rate, p.device_block),
        (96_000, 96_000, 64)
    );
}

#[test]
fn a_rate_apps_may_not_ask_has_no_plan() {
    for rate in [0, 22_050, 32_000, 88_200, 192_000] {
        assert_eq!(Plan::new(96_000, rate, 32), None, "{rate}");
    }
    // Above the device's rate, no block, no rate, or a block the shared
    // memory cannot carry.
    assert_eq!(Plan::new(48_000, 96_000, 32), None);
    assert_eq!(Plan::new(96_000, 48_000, 0), None);
    assert_eq!(Plan::new(0, 0, 32), None);
    assert_eq!(Plan::new(96_000, 96_000, shm::MAX_APP_BLOCK + 1), None);
    assert!(Plan::new(96_000, 96_000, shm::MAX_APP_BLOCK).is_some());
    // The device's own rate always serves, a listed one or not.
    let own = Plan::new(88_200, 88_200, 32).unwrap();
    assert_eq!((own.app_block, own.exact), (32, true));
    let low = Plan::new(48_000, 48_000, 32).unwrap();
    assert!(!low.converts());
    assert_eq!(Plan::new(48_000, 44_100, 32).unwrap().app_block, 30);
}

#[test]
fn an_odd_device_block_is_not_exact_at_half_the_rate() {
    // Not a pump block, but the rule holds the half-band's own condition.
    let p = Plan::new(96_000, 48_000, 33).unwrap();
    assert_eq!((p.app_block, p.exact), (17, false));
}

#[test]
fn exact_is_the_converter_s_own_half_band_for_every_rate_and_block() {
    for &block in &shm::BLOCKS {
        for &rate in &shm::APP_RATES {
            let plan = Plan::new(96_000, rate, block).unwrap();
            if !plan.converts() {
                assert!(plan.exact);
                continue;
            }
            let c = Converter::low_delay(96_000, rate, 1, block as usize, Fixed::Input).unwrap();
            assert_eq!(plan.exact, c.is_half_band(), "{rate} Hz at block {block}");
            // An exact plan gets exactly its block from every device block.
            if plan.exact {
                assert_eq!(c.output_frames_max(), plan.app_block as usize);
            }
        }
    }
}

#[test]
fn the_prime_is_a_block_only_where_the_ratio_is_not_exact() {
    assert_eq!(Plan::new(96_000, 48_000, 32).unwrap().prime(), 0);
    assert_eq!(Plan::new(96_000, 96_000, 32).unwrap().prime(), 0);
    assert_eq!(Plan::new(96_000, 44_100, 128).unwrap().prime(), 59);
}

#[test]
fn the_prime_is_what_dlink_s_fixed_app_block_adds_to_its_tx_side() {
    // dlink's fixed app block adds its prime to the TX FIFO beyond the
    // free `delay_ns` (which `conversion_ns` reports), and gives the plan's
    // block: the latencies stay the path's.
    for &block in &shm::BLOCKS {
        for &rate in &shm::APP_RATES {
            let plan = Plan::new(96_000, rate, block).unwrap();
            if !plan.converts() {
                continue;
            }
            let rated =
                Rated::with_fixed_app_block(Echo, 96_000, rate, block as usize, 1, 1).unwrap();
            let what = format!("{rate} Hz at block {block}");
            assert_eq!(rated.app_block(), Some(plan.app_block as usize), "{what}");
            let (_, free_tx) = conversion_ns(&plan).unwrap();
            let (_, fixed_tx) = rated.delay_ns();
            let prime_ns = plan.prime() as u64 * 1_000_000_000 / u64::from(rate);
            assert!(
                fixed_tx.abs_diff(free_tx + prime_ns) <= 1,
                "{what}: TX {fixed_tx} ns, free {free_tx} + prime {prime_ns}"
            );
        }
    }
}

#[test]
fn samples_and_ns_go_to_app_samples_rounded_up_the_wait_rounded_down() {
    let p = Plan::new(96_000, 44_100, 128).unwrap();
    // 100 * 0.459375 = 45.9375
    assert_eq!(p.to_app(100), 46);
    assert_eq!(p.to_app(0), 0);
    // 320 device samples are exactly 147 app samples.
    assert_eq!(p.to_app(320), 147);
    assert_eq!(p.wait_offset(320), 147);
    assert_eq!(p.wait_offset(100), 45);
    // A device time on the app's clock rounds down too: 1152 * 0.459375
    // = 529.2.
    assert_eq!(p.at_app(9 * 128), 529);
    assert_eq!(p.at_app(320), 147);
    // 1 ms at 44.1 kHz: 44.1 samples.
    assert_eq!(p.ns_to_app(1_000_000), 45);
    assert_eq!(p.ns_to_app(0), 0);
    let h = Plan::new(96_000, 48_000, 32).unwrap();
    assert_eq!(h.to_app(33), 17);
    assert_eq!(h.wait_offset(33), 16);
    assert_eq!(h.ns_to_app(1_000_000), 48);
    assert_eq!(h.ns_to_app(1_000_001), 49);
    // The DAW block's period: 16 at 48 kHz is the device's 32 at 96.
    assert_eq!(h.period_ns(), 333_333);
    assert_eq!(p.period_ns(), 59 * 1_000_000_000 / 44_100);
}

#[test]
fn the_reported_latencies_are_the_device_s_at_the_app_rate_plus_the_conversion() {
    // At the device's rate: the device's own, whatever the converter.
    let same = Plan::new(96_000, 96_000, 32).unwrap();
    assert_eq!(same.latencies((128, 152), (0, 0)), (128, 152));
    assert_eq!(same.latencies((128, 152), (5_000, 5_000)), (128, 152));
    // 48 kHz: B + P = 128 -> 64, out 152 -> 76; 0.63 ms -> 30.24 -> 31,
    // 0.64 ms -> 30.72 -> 31; exact: no fixed block share.
    let h = Plan::new(96_000, 48_000, 32).unwrap();
    assert_eq!(h.fixed_block_share(), (0, 0));
    assert_eq!(h.latencies((128, 152), (630_000, 640_000)), (95, 107));
    // 44.1 kHz at block 128: A = 59, the share 29 in and 30 out (one
    // block over the round trip, the TX FIFO's prime); 224 -> 102.9 -> 103,
    // 1.32 ms -> 58.21 -> 59; 400 -> 183.75 -> 184, 3.08 ms -> 135.83
    // -> 136.
    let p = Plan::new(96_000, 44_100, 128).unwrap();
    assert_eq!(p.fixed_block_share(), (29, 30));
    let (i, o) = p.latencies((224, 400), (1_320_000, 3_080_000));
    assert_eq!((i, o), (103 + 59 + 29, 184 + 136 + 30));
    // The shares add up to the prime: the round trip's one block.
    let (a, b) = p.fixed_block_share();
    assert_eq!(a + b, p.prime() as u64);
}

#[test]
fn a_huge_latency_saturates_instead_of_wrapping() {
    let p = Plan::new(96_000, 48_000, 32).unwrap();
    assert_eq!(p.latencies((u32::MAX, 0), (u64::MAX / 2, 0)).0, u32::MAX);
}

#[test]
fn a_switch_comes_only_for_another_supported_rate() {
    let now = Plan::new(96_000, 96_000, 32).unwrap();
    assert_eq!(switch_to(0, &now), None);
    assert_eq!(switch_to(96_000, &now), None);
    assert_eq!(switch_to(22_050, &now), None);
    assert_eq!(switch_to(48_000, &now), Plan::new(96_000, 48_000, 32));
    let at48 = Plan::new(96_000, 48_000, 32).unwrap();
    assert_eq!(switch_to(48_000, &at48), None);
    assert_eq!(switch_to(96_000, &at48), Some(now));
    assert_eq!(switch_to(44_100, &at48), Plan::new(96_000, 44_100, 32));
}

#[test]
fn the_first_plan_is_the_rate_asked_before_else_the_device_s() {
    assert_eq!(
        first_plan(96_000, 64, 48_000),
        Plan::new(96_000, 48_000, 64)
    );
    assert_eq!(first_plan(96_000, 64, 0), Plan::new(96_000, 96_000, 64));
    assert_eq!(
        first_plan(96_000, 64, 12_345),
        Plan::new(96_000, 96_000, 64)
    );
    assert_eq!(first_plan(96_000, 0, 48_000), None);
}

/// Answers each block with its inputs (an echo).
#[derive(Default)]
struct Echo;

impl Process for Echo {
    fn process(&mut self, block: &mut Block<'_>) {
        for c in 0..block.outputs().min(block.inputs()) {
            let input = block.input(c).to_vec();
            block.output(c).copy_from_slice(&input);
        }
    }
}

/// The device-side delay of a pulse through `Served` around an echo (the
/// DAW answering in the same block): device samples. The pulse is a
/// Gaussian 8 samples wide, far inside both rates' band, so the filters
/// pass its shape and its peak moves by their delay alone.
fn served_delay(plan: &Plan) -> usize {
    let (mut s, _) = Served::new(Echo, plan, 1, 1, WaitFrom::default()).unwrap();
    let n = plan.device_block as usize;
    let at = 50 * n + 7;
    let pulse = |i: usize| {
        let x = (i as f64 - at as f64) / 8.0;
        0.5 * (-0.5 * x * x).exp()
    };
    let mut out = Vec::new();
    // The pulse at block 50 and the longest path (44.1 kHz at block 32:
    // about 9 ms, 27 blocks) are out by block 120.
    for b in 0..120 {
        let input: Vec<f64> = (0..n).map(|i| pulse(b * n + i)).collect();
        let mut o = vec![0.0; n];
        s.process(&mut Block::new(n, &input, &mut o).at((b * n) as u64));
        out.extend_from_slice(&o);
    }
    let peak = (0..out.len())
        .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
        .unwrap();
    assert!(out[peak].abs() > 0.3, "{plan:?}: peak {}", out[peak]);
    peak - at
}

#[test]
fn the_converted_path_delays_what_the_latencies_say_beyond_the_device() {
    for (rate, block) in [(48_000, 32), (48_000, 128), (44_100, 32), (44_100, 128)] {
        let plan = Plan::new(96_000, rate, block).unwrap();
        let measured = served_delay(&plan) as f64 * f64::from(rate) / 96_000.0;
        // What the latencies add beyond the device's own terms (the echo
        // answers in its own block: no double buffer here).
        let (rx, tx) = conversion_ns(&plan).unwrap();
        let (a, b) = plan.fixed_block_share();
        let said = (plan.ns_to_app(rx) + plan.ns_to_app(tx) + a + b) as f64;
        assert!(
            (measured - said).abs() <= 4.0,
            "{rate} Hz at block {block}: measured {measured} app samples, the latencies say {said}"
        );
    }
}

#[test]
fn the_device_s_rate_is_the_bridge_alone_without_delay() {
    let plan = Plan::new(96_000, 96_000, 32).unwrap();
    let (s, counters) = Served::new(Echo, &plan, 1, 1, WaitFrom::default()).unwrap();
    assert!(matches!(s, Served::Direct(_)));
    assert_eq!(counters.read(), 0);
    assert_eq!(served_delay(&plan), 0);
    assert_eq!(conversion_ns(&plan).unwrap(), (0, 0));
}

#[test]
fn the_counters_read_the_converter_s_dry_frames() {
    let dry = std::sync::Arc::new(AtomicU64::new(7));
    let counters = Counters {
        converter_dry: Some(std::sync::Arc::clone(&dry)),
    };
    assert_eq!(counters.read(), 7);
    dry.fetch_add(5, Ordering::Relaxed);
    assert_eq!(counters.read(), 12);
    assert_eq!(Counters::default().read(), 0);
}

/// Counts the discontinuities it gets (shared, so the test reads them
/// through the converters' box).
struct Resets(std::sync::Arc<AtomicU64>);

impl Process for Resets {
    fn process(&mut self, _block: &mut Block<'_>) {}

    fn discontinuity(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_discontinuity_reaches_the_bridge_on_either_path() {
    for rate in [96_000, 48_000, 44_100] {
        let plan = Plan::new(96_000, rate, 32).unwrap();
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let (mut s, _) = Served::new(
            Resets(std::sync::Arc::clone(&seen)),
            &plan,
            1,
            1,
            WaitFrom::default(),
        )
        .unwrap();
        s.discontinuity();
        s.discontinuity();
        assert_eq!(seen.load(Ordering::Relaxed), 2, "{rate} Hz");
    }
}

#[test]
fn a_converted_path_counts_nothing_dry_in_steady_running() {
    for rate in [48_000, 44_100] {
        let plan = Plan::new(96_000, rate, 32).unwrap();
        let (mut s, counters) = Served::new(Echo, &plan, 2, 2, WaitFrom::default()).unwrap();
        assert!(matches!(s, Served::Converted(_)));
        // 100 blocks: the filters are full after about ten.
        for b in 0..100u64 {
            let input = [0.25; 2 * 32];
            let mut o = [0.0; 2 * 32];
            s.process(&mut Block::new(32, &input, &mut o).at(b * 32));
        }
        assert_eq!(counters.read(), 0, "{rate}");
    }
}

/// Writes 0.5 to its outputs for its first `loud` calls, then leaves them
/// alone, as the bridge does with no DAW attached or deep in a gap.
struct Quiet {
    loud: u32,
}

impl Process for Quiet {
    fn process(&mut self, block: &mut Block<'_>) {
        if self.loud == 0 {
            return;
        }
        self.loud -= 1;
        for c in 0..block.outputs() {
            block.output(c).fill(0.5);
        }
    }
}

#[test]
fn a_block_the_bridge_leaves_alone_plays_silence_on_either_path() {
    // The pump zeroes the outputs before each block, and the bridge writes
    // nothing where it has nothing to play (no DAW, a gap's later blocks).
    // Behind the converters it must be the same: never the last block
    // again.
    for rate in [96_000, 48_000, 44_100] {
        let plan = Plan::new(96_000, rate, 32).unwrap();
        let (mut s, _) = Served::new(Quiet { loud: 20 }, &plan, 1, 1, WaitFrom::default()).unwrap();
        let input = [0.0; 32];
        let mut tail = 0.0f64;
        // 20 loud calls, then the filters' tail (well under 100 blocks),
        // then 100 blocks that must be silent.
        for b in 0..300u64 {
            let mut o = [0.0; 32];
            s.process(&mut Block::new(32, &input, &mut o).at(b * 32));
            if b >= 200 {
                tail = o.iter().fold(tail, |m, y| m.max(y.abs()));
            }
        }
        assert!(tail < 1e-9, "{rate} Hz: the last block again ({tail})");
    }
}

/// A DAW on a scripted media clock (app samples, one sample a read): once
/// the clock reaches `at` it has answered every block before the newest one
/// handed over (the one the bridge waits for, without a spare block).
struct Scripted {
    seq: std::sync::Arc<AtomicU64>,
    ack: std::sync::Arc<AtomicU64>,
    late: std::sync::Arc<AtomicU64>,
}

impl crate::bridge::Link for Scripted {
    fn attached(&self) -> bool {
        true
    }
    fn input(&mut self, _half: u32, _ch: usize) -> &mut [i32] {
        &mut []
    }
    fn read_output(&self, _half: u32, _ch: usize, dst: &mut [f64]) {
        dst.fill(0.5);
    }
    fn publish(&self, seq: u64, _start: Option<u64>) {
        self.seq.store(seq, Ordering::Relaxed);
    }
    fn ack(&self) -> u64 {
        self.ack.load(Ordering::Relaxed)
    }
    fn count_late(&self) {
        self.late.fetch_add(1, Ordering::Relaxed);
    }
}

/// The bridge behind the converted path, counting its blocks that start
/// more than 2 app frames before the device block they run in (`wait_from`,
/// set by `Served` before it runs that device block).
struct EarlyStarts<P> {
    inner: P,
    wait_from: WaitFrom,
    early: std::sync::Arc<AtomicU64>,
}

impl<P: Process> Process for EarlyStarts<P> {
    fn process(&mut self, block: &mut Block<'_>) {
        let early = match (block.start(), self.wait_from.get()) {
            (Some(s), Some(w)) => s + 2 < w,
            _ => false,
        };
        if early {
            self.early.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.process(block);
    }
}

#[test]
fn behind_the_fixed_app_block_the_bridge_waits_until_the_device_block_s_deadline() {
    use std::sync::Arc;
    // 44.1 kHz on a 128-sample device block: 58 or 59 app frames a device
    // block, a DAW block of 59 (dlink's fixed app block). A DAW block
    // starts up to 58 frames before the device block that completes it.
    const OFFSET: u64 = 40;
    let plan = Plan::new(96_000, 44_100, 128).unwrap();
    let (seq, ack, late) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );
    let (clock, at) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let now = {
        let (clock, at, seq, ack) = (
            Arc::clone(&clock),
            Arc::clone(&at),
            Arc::clone(&seq),
            Arc::clone(&ack),
        );
        move || {
            let t = clock.fetch_add(1, Ordering::Relaxed);
            if t >= at.load(Ordering::Relaxed) {
                ack.fetch_max(
                    seq.load(Ordering::Relaxed).saturating_sub(1),
                    Ordering::Relaxed,
                );
            }
            Some(t)
        }
    };
    let link = Scripted {
        seq: Arc::clone(&seq),
        ack: Arc::clone(&ack),
        late: Arc::clone(&late),
    };
    let bridge = crate::bridge::Bridge::with_spare(link, 0).with_deadline(OFFSET, now);
    let waited = bridge.waited_counter();
    let wait_from = bridge.wait_from();
    let early = Arc::new(AtomicU64::new(0));
    let inner = EarlyStarts {
        inner: bridge,
        wait_from: wait_from.clone(),
        early: Arc::clone(&early),
    };
    let (mut s, _) = Served::new(inner, &plan, 1, 1, wait_from).unwrap();
    for b in 0..300u64 {
        // The device block starts now on the app's clock; the DAW answers
        // 2 samples before its deadline (its start + the offset).
        let start = plan.at_app(b * 128);
        clock.store(start, Ordering::Relaxed);
        at.store(start + OFFSET - 2, Ordering::Relaxed);
        let input = [0.0; 128];
        let mut o = [0.0; 128];
        s.process(&mut Block::new(128, &input, &mut o).at(b * 128));
    }
    let early = early.load(Ordering::Relaxed);
    assert!(early > 200, "the case under test: {early}");
    // Every answer came before its device block's deadline: none late.
    assert_eq!(late.load(Ordering::Relaxed), 0, "late answers");
    assert!(waited.load(Ordering::Relaxed) > 250, "saved by the wait");
}

/// Counts its calls and those not of `want` frames (shared, so the test
/// reads them through the converters' box).
struct Sizes {
    want: usize,
    calls: std::sync::Arc<AtomicU64>,
    other: std::sync::Arc<AtomicU64>,
}

impl Process for Sizes {
    fn process(&mut self, block: &mut Block<'_>) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if block.frames() != self.want {
            self.other.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[test]
fn the_daw_gets_its_fixed_block_at_most_once_a_device_block() {
    for (rate, block, channels) in [
        (44_100, 128, 1),
        (44_100, 32, 2),
        (44_100, 32, 0),
        (48_000, 32, 1),
        (48_000, 128, 0),
    ] {
        let plan = Plan::new(96_000, rate, block).unwrap();
        let (calls, other) = (
            std::sync::Arc::new(AtomicU64::new(0)),
            std::sync::Arc::new(AtomicU64::new(0)),
        );
        let sizes = Sizes {
            want: plan.app_block as usize,
            calls: std::sync::Arc::clone(&calls),
            other: std::sync::Arc::clone(&other),
        };
        let (mut s, counters) =
            Served::new(sizes, &plan, channels, channels, WaitFrom::default()).unwrap();
        let n = block as usize;
        let input = vec![0.25; channels * n];
        let mut o = vec![0.0; channels * n];
        let blocks = 1_000u64;
        let mut most = 0;
        for b in 0..blocks {
            let before = calls.load(Ordering::Relaxed);
            s.process(&mut Block::new(n, &input, &mut o).at(b * u64::from(block)));
            most = most.max(calls.load(Ordering::Relaxed) - before);
        }
        let what = format!("{rate} Hz at block {block}, {channels} channels");
        assert_eq!(other.load(Ordering::Relaxed), 0, "{what}: other sizes");
        assert_eq!(most, 1, "{what}: calls in one device block");
        // The DAW keeps the app's pace: the device's frames at its rate
        // over its block, to a call (the RX FIFO holds less than one).
        let frames = plan.to_app(blocks * u64::from(block));
        let calls = calls.load(Ordering::Relaxed);
        let want = frames / u64::from(plan.app_block);
        assert!(
            calls + 1 >= want && calls <= want + 1,
            "{what}: {calls} calls for {want}"
        );
        if plan.exact {
            assert_eq!(calls, blocks, "{what}: one call a device block");
        }
        assert_eq!(counters.read(), 0, "{what}: dry frames");
    }
}

#[test]
fn the_converted_path_sets_the_device_block_s_start_for_the_wait() {
    for rate in [96_000, 48_000, 44_100] {
        let plan = Plan::new(96_000, rate, 128).unwrap();
        let wait_from = WaitFrom::default();
        let (mut s, _) = Served::new(Echo, &plan, 1, 1, wait_from.clone()).unwrap();
        let input = [0.0; 128];
        let mut o = [0.0; 128];
        s.process(&mut Block::new(128, &input, &mut o).at(0));
        let first = wait_from.get();
        for b in 1..10u64 {
            s.process(&mut Block::new(128, &input, &mut o).at(b * 128));
        }
        let tenth = wait_from.get();
        if plan.converts() {
            // The device block's start in app samples: 0, then the tenth
            // block's 9 * 128 device samples at the app's rate (on the
            // app's clock, rounded down: `Plan::at_app`).
            assert_eq!(first, Some(0), "{rate} Hz");
            // 44.1 kHz: 1152 * 0.459375 = 529.2 -> 529; 48 kHz: 576.
            let want = [(44_100, 529), (48_000, 576)]
                .into_iter()
                .find(|&(r, _)| r == rate)
                .map(|(_, w)| w);
            assert_eq!(tenth, want, "{rate} Hz");
        } else {
            // The bridge alone: nothing sets the cell (the direct path
            // never sees it), so the bridge waits from its own block's
            // start; this also pins `WaitFrom`'s unset value.
            assert_eq!((first, tenth), (None, None), "{rate} Hz");
        }
    }
}
