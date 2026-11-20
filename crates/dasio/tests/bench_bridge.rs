// SPDX-License-Identifier: GPL-3.0-or-later

//! The bridge's cost per block at FOH's shape (dasio#28 E13, the bench):
//! 64 inputs and 64 outputs of 256 samples at 96 kHz, one spare block,
//! concealment and a deadline, as the band instance runs. Prints the mean
//! and the worst block. In an optimized build (CI's `cargo test --release`
//! step, the build the PCs run) it fails when the mean takes more than a
//! tenth of the block's period (2.67 ms): the pump's work margin is the
//! rest. A debug build costs several times more per sample (562–701 us a
//! block on the CI runners, 10.11.2026), so there it only checks that the
//! answers play.
//!
//! The e2e-shape bench (dasio#107) times the 44.1 kHz conversion at the
//! e2e test's instance shape and holds it in every build, since the e2e
//! test runs the debug (and coverage-instrumented) binary. The FOH-shape
//! conversion bench (dasio#107) times the 44.1 and 48 kHz conversion at
//! FOH's shape and holds it in the optimized build only, as the FOH bench
//! holds the bridge.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dasio::app_rate::{Plan, Served};
use dasio::bridge::{Bridge, Link, to_f64};
use dstack_audio_io::{Block, Process};
use dstack_resample::{Converter, Fixed};

const FRAMES: usize = 256;
const CHANNELS: usize = 64;
const RATE: u64 = 96_000;
const BLOCKS: u64 = 2_000;

/// A stand-in DAW that answers every block at once with its inputs.
struct Daw {
    inputs: [Vec<Vec<i32>>; 2],
    outputs: [Vec<Vec<AtomicI32>>; 2],
    ack: AtomicU64,
    attached: AtomicBool,
}

impl Link for Daw {
    fn attached(&self) -> bool {
        self.attached.load(Ordering::Relaxed)
    }

    fn input(&mut self, half: u32, ch: usize) -> &mut [i32] {
        &mut self.inputs[half as usize][ch]
    }

    fn read_output(&self, half: u32, ch: usize, dst: &mut [f64]) {
        for (y, s) in dst.iter_mut().zip(&self.outputs[half as usize][ch]) {
            *y = to_f64(s.load(Ordering::Relaxed));
        }
    }

    fn publish(&self, seq: u64, _start: Option<u64>) {
        let half = (seq % 2) as usize;
        for (outs, ins) in self.outputs[half].iter().zip(&self.inputs[half]) {
            for (o, &i) in outs.iter().zip(ins) {
                o.store(i, Ordering::Relaxed);
            }
        }
        self.ack.store(seq, Ordering::Release);
    }

    fn ack(&self) -> u64 {
        self.ack.load(Ordering::Acquire)
    }

    fn count_late(&self) {}
}

/// The benches take turns: the test harness runs tests on parallel
/// threads, and a bench timed beside another measures the other's load
/// too (CI 43807646396: the FOH bench beside the e2e-shape one).
static ALONE: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    ALONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `n` samples of a sawtooth-like test signal, ±0.5 full scale.
fn signal(n: usize) -> Vec<f64> {
    (0..n).map(|i| ((i % 997) as f64 / 997.0) - 0.5).collect()
}

fn channels(n: usize, frames: usize) -> Vec<Vec<i32>> {
    (0..n).map(|_| vec![0; frames]).collect()
}

fn atomics(n: usize, frames: usize) -> Vec<Vec<AtomicI32>> {
    (0..n)
        .map(|_| (0..frames).map(|_| AtomicI32::new(0)).collect())
        .collect()
}

/// A stand-in DAW of `inputs` and `outputs` channels of `frames` samples,
/// attached.
fn daw(inputs: usize, outputs: usize, frames: usize) -> Daw {
    Daw {
        inputs: [channels(inputs, frames), channels(inputs, frames)],
        outputs: [atomics(outputs, frames), atomics(outputs, frames)],
        ack: AtomicU64::new(0),
        attached: AtomicBool::new(true),
    }
}

#[test]
fn a_block_at_foh_s_shape_takes_a_small_part_of_its_period() {
    let _alone = alone();
    let daw = daw(CHANNELS, CHANNELS, FRAMES);
    let now = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&now);
    let mut b = Bridge::with_spare(daw, 1)
        .with_deadline(FRAMES as u64, move || Some(clock.load(Ordering::Relaxed)))
        .with_concealment(CHANNELS, FRAMES);
    let input: Vec<f64> = signal(CHANNELS * FRAMES);
    let mut output = vec![0.0f64; CHANNELS * FRAMES];
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    let mut start = 0u64;
    for _ in 0..BLOCKS {
        start += FRAMES as u64;
        now.store(start, Ordering::Relaxed);
        let mut block = Block::new(FRAMES, &input, &mut output).at(start);
        let t = Instant::now();
        b.process(&mut block);
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    let period = Duration::from_nanos(FRAMES as u64 * 1_000_000_000 / RATE);
    let mean = total / BLOCKS as u32;
    eprintln!(
        "bridge at {CHANNELS}x{FRAMES}: mean {mean:?}, worst {worst:?}, period {period:?} ({:.2} % mean)",
        mean.as_secs_f64() * 100.0 / period.as_secs_f64()
    );
    if !cfg!(debug_assertions) {
        assert!(
            mean < period / 10,
            "mean {mean:?} per block, more than a tenth of the {period:?} period"
        );
    }
    // The answers came through: the last block's outputs are the DAW's.
    assert!(output.iter().any(|&y| y != 0.0), "the DAW's answers played");
}

/// The e2e test's instance at 44.1 kHz (dasio#107): 32 RX and 16 TX
/// channels on 32-frame device blocks at 96 kHz.
const E2E_RX: usize = 32;
const E2E_TX: usize = 16;
const E2E_BLOCK: usize = 32;
const E2E_BLOCKS: u64 = 3_000;

/// Mean and worst of `blocks` calls of `f` with each block's start, for
/// blocks of `frames` samples.
fn time_blocks(blocks: u64, frames: usize, mut f: impl FnMut(u64)) -> (Duration, Duration) {
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for i in 0..blocks {
        let t = Instant::now();
        f(i * frames as u64);
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    (total / blocks as u32, worst)
}

/// The instance's processor for `plan` with `rx` RX and `tx` TX channels,
/// as `run::serve` builds it: the bridge (one spare block, a deadline,
/// concealment) on a stand-in DAW that answers at once, behind the
/// conversion at another rate. The DAW never makes the bridge wait, so the
/// deadline's offset (a block here, `Plan::wait_offset` in the service)
/// never binds.
fn served(plan: &Plan, rx: usize, tx: usize) -> (Served<Bridge<Daw>>, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&now);
    let app_block = plan.app_block as usize;
    let bridge = Bridge::with_spare(daw(rx, tx, app_block), 1)
        .with_deadline(app_block as u64, move || {
            Some(clock.load(Ordering::Relaxed))
        })
        .with_concealment(tx, app_block);
    let wait_from = bridge.wait_from();
    let (served, _) = Served::new(bridge, plan, rx, tx, wait_from).expect("the plan serves");
    (served, now)
}

/// Mean and worst of the instance's processor for `plan` (`rx` RX and `tx`
/// TX channels) per device block over `blocks` device blocks of the plan's
/// `device_block` frames, and whether its outputs played the DAW's answers.
fn time_served(plan: &Plan, rx: usize, tx: usize, blocks: u64) -> (Duration, Duration, bool) {
    let (mut p, now) = served(plan, rx, tx);
    let frames = plan.device_block as usize;
    let input: Vec<f64> = signal(rx * frames);
    let mut output = vec![0.0f64; tx * frames];
    let mut played = false;
    let ratio = f64::from(plan.app_rate) / f64::from(plan.device_rate);
    let (mean, worst) = time_blocks(blocks, frames, |start| {
        now.store((start as f64 * ratio) as u64, Ordering::Relaxed);
        let mut block = Block::new(frames, &input, &mut output).at(start);
        p.process(&mut block);
        played |= output.iter().any(|&y| y != 0.0);
    });
    (mean, worst, played)
}

/// Mean and worst of the two converters alone per device block, at the
/// e2e shape (96 kHz to 44.1 kHz for RX, back for TX).
fn time_converters() -> (Duration, Duration) {
    let mut rx = Converter::low_delay(96_000, 44_100, E2E_RX, E2E_BLOCK, Fixed::Input)
        .expect("the RX converter");
    let mut tx = Converter::low_delay(44_100, 96_000, E2E_TX, E2E_BLOCK, Fixed::Output)
        .expect("the TX converter");
    let room = rx.output_frames_max();
    let rx_in: Vec<f64> = signal(E2E_RX * E2E_BLOCK);
    let mut rx_out = vec![0.0f64; E2E_RX * room];
    let tx_in = vec![0.25f64; E2E_TX * tx.input_frames_max()];
    let mut tx_out = vec![0.0f64; E2E_TX * E2E_BLOCK];
    time_blocks(E2E_BLOCKS, E2E_BLOCK, |_| {
        rx.process(&rx_in, E2E_BLOCK, &mut rx_out, room)
            .expect("RX converts");
        let need = tx.input_frames_next();
        tx.process(&tx_in[..E2E_TX * need], need, &mut tx_out, E2E_BLOCK)
            .expect("TX converts");
    })
}

/// The e2e test runs the debug (or coverage-instrumented) dasio binary,
/// and its 44.1 kHz step failed when the converted path overran the 333 us
/// device block (CI 43806346111: max_process 456-483 us, 49-64 overruns a
/// second, dasio#107). The conversion's own cost (the 44.1 kHz path less
/// the plain bridge at the same shape, which the 96 kHz e2e steps already
/// carry) holds in every build the tests run: a quarter of the device
/// block's period on average, so the device's own work and the runner's
/// jitter keep the rest. The subtraction removes a bridge of 32 frames a
/// block where the converted path runs about 14.7, so it overstates the
/// bridge's share by a few us. The two converters alone are printed beside
/// it, so a failure says which part costs.
#[test]
fn the_44_1_khz_conversion_at_the_e2e_shape_takes_a_small_part_of_the_device_block() {
    let _alone = alone();
    let period = Duration::from_nanos(E2E_BLOCK as u64 * 1_000_000_000 / RATE);
    let direct = Plan::new(96_000, 96_000, E2E_BLOCK as u32).expect("96 kHz");
    let converted = Plan::new(96_000, 44_100, E2E_BLOCK as u32).expect("44.1 kHz");
    let (d_mean, d_worst, _) = time_served(&direct, E2E_RX, E2E_TX, E2E_BLOCKS);
    let (c_mean, c_worst) = time_converters();
    let (mean, worst, played) = time_served(&converted, E2E_RX, E2E_TX, E2E_BLOCKS);
    let conversion = mean.saturating_sub(d_mean);
    eprintln!(
        "e2e shape {E2E_RX}/{E2E_TX}x{E2E_BLOCK} at 96 kHz, per device block: \
         bridge alone mean {d_mean:?} worst {d_worst:?}; \
         converters alone mean {c_mean:?} worst {c_worst:?}; \
         44.1 kHz path mean {mean:?} worst {worst:?}; \
         the conversion's own {conversion:?}; period {period:?}"
    );
    assert!(
        conversion < period / 4,
        "the 44.1 kHz conversion adds {conversion:?} per device block on average, more than a quarter of the {period:?} period \
         (path {mean:?}, bridge alone {d_mean:?}, converters alone {c_mean:?})"
    );
    assert!(played, "the DAW's answers played through the conversion");
}

/// The conversion's cost at FOH's shape (dasio#107, the CI half of the
/// field test): the band instance's 64 RX and 64 TX channels on 256-frame
/// device blocks at 96 kHz, with an app at 44.1 or 48 kHz. For each rate
/// the plain bridge (the 96 kHz plan) and the converted path are timed one
/// after the other, so each subtraction compares runs on the same runner
/// state. The conversion's own cost (the path less the bridge) holds in
/// the optimized build, the one the PCs run: a quarter of the device
/// block's period (2.67 ms) on average, the e2e bench's bar. A debug build
/// only checks that the answers play, as the FOH bench does. The
/// subtraction removes a bridge of 256 frames a block where the converted
/// path runs about 117.6 (44.1 kHz) or 128 (48 kHz) app frames, so it
/// overstates the bridge's share, and understates the conversion's, by
/// about half the bridge's mean, which is printed beside it.
#[test]
fn the_conversion_at_foh_s_shape_takes_a_small_part_of_the_device_block() {
    let _alone = alone();
    let period = Duration::from_nanos(FRAMES as u64 * 1_000_000_000 / RATE);
    let direct = Plan::new(96_000, 96_000, FRAMES as u32).expect("96 kHz");
    for rate in [44_100, 48_000] {
        let converted = Plan::new(96_000, rate, FRAMES as u32).expect("the app's rate");
        let (d_mean, d_worst, _) = time_served(&direct, CHANNELS, CHANNELS, BLOCKS);
        let (mean, worst, played) = time_served(&converted, CHANNELS, CHANNELS, BLOCKS);
        let conversion = mean.saturating_sub(d_mean);
        eprintln!(
            "FOH shape {CHANNELS}/{CHANNELS}x{FRAMES} at 96 kHz, app at {rate} Hz \
             (DAW block {}), per device block: \
             bridge alone mean {d_mean:?} worst {d_worst:?}; \
             path mean {mean:?} worst {worst:?}; \
             the conversion's own {conversion:?} ({:.2} % of the period); period {period:?}",
            converted.app_block,
            conversion.as_secs_f64() * 100.0 / period.as_secs_f64()
        );
        if !cfg!(debug_assertions) {
            assert!(
                conversion < period / 4,
                "the {rate} Hz conversion adds {conversion:?} per device block on average, more than a quarter of the {period:?} period \
                 (path {mean:?}, bridge alone {d_mean:?})"
            );
        }
        assert!(
            played,
            "the DAW's answers played through the {rate} Hz conversion"
        );
    }
}
