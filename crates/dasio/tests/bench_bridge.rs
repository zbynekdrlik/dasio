// SPDX-License-Identifier: GPL-3.0-or-later

//! The bridge's cost per block at FOH's shape (dasio#28 E13, the bench):
//! 64 inputs and 64 outputs of 256 samples at 96 kHz, one spare block,
//! concealment and a deadline, as the band instance runs. Prints the mean
//! and the worst block, and fails when the mean takes more than a tenth
//! of the block's period (2.67 ms): the pump's work margin is the rest.
//! A debug build on a shared runner, so the bound is loose; a regression
//! that makes the bridge do per-sample work it should not still trips it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dasio::bridge::{Bridge, Link, to_f64};
use dstack_audio_io::{Block, Process};

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

fn channels() -> Vec<Vec<i32>> {
    (0..CHANNELS).map(|_| vec![0; FRAMES]).collect()
}

fn atomics() -> Vec<Vec<AtomicI32>> {
    (0..CHANNELS)
        .map(|_| (0..FRAMES).map(|_| AtomicI32::new(0)).collect())
        .collect()
}

#[test]
fn a_block_at_foh_s_shape_takes_a_small_part_of_its_period() {
    let daw = Daw {
        inputs: [channels(), channels()],
        outputs: [atomics(), atomics()],
        ack: AtomicU64::new(0),
        attached: AtomicBool::new(true),
    };
    let now = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&now);
    let mut b = Bridge::with_spare(daw, 1)
        .with_deadline(FRAMES as u64, move || Some(clock.load(Ordering::Relaxed)))
        .with_concealment(CHANNELS, FRAMES);
    let input: Vec<f64> = (0..CHANNELS * FRAMES)
        .map(|i| ((i % 997) as f64 / 997.0) - 0.5)
        .collect();
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
    assert!(
        mean < period / 10,
        "mean {mean:?} per block, more than a tenth of the {period:?} period"
    );
    // The answers came through: the last block's outputs are the DAW's.
    assert!(output.iter().any(|&y| y != 0.0), "the DAW's answers played");
}
