// SPDX-License-Identifier: GPL-3.0-or-later

//! The bridge's block under RealtimeSanitizer (dasio#28 E13). The `rtsan` CI
//! job builds this test with a pinned nightly, `-Zsanitizer=realtime`,
//! `-Zbuild-std` and `--cfg dasio_rtsan`; `Bridge::process` then carries
//! `#[sanitize(realtime = "nonblocking")]`, and any allocation, lock or
//! blocking syscall inside it aborts the test binary. Built normally (every
//! other job), the same workload runs without the sanitizer. The stand-in
//! DAW keeps its buffers in atomics, so the sanitizer judges the bridge's
//! own path, not the test's.
#![cfg_attr(dasio_rtsan, feature(sanitize))]

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use dasio::bridge::{Bridge, Link, to_f64};
use dasio::miss_watch::{Config, Kind, Setup, ToneKind, Watched};
use dstack_audio_io::{Block, Process};

const FRAMES: usize = 32;
const CHANNELS: usize = 2;
const PROBE: &str = "DASIO_RTSAN_PROBE";

/// A stand-in DAW, lock-free: it answers each block with its inputs
/// negated while `answers` is set.
struct Daw {
    attached: Arc<AtomicBool>,
    answers: Arc<AtomicBool>,
    inputs: [Vec<i32>; 2],
    outputs: [Vec<AtomicI32>; 2],
    ack: AtomicU64,
    late: Arc<AtomicU64>,
}

impl Link for Daw {
    fn attached(&self) -> bool {
        self.attached.load(Ordering::Relaxed)
    }

    fn input(&mut self, half: u32, _ch: usize) -> &mut [i32] {
        &mut self.inputs[half as usize]
    }

    fn read_output(&self, half: u32, _ch: usize, dst: &mut [f64]) {
        for (y, s) in dst.iter_mut().zip(&self.outputs[half as usize]) {
            *y = to_f64(s.load(Ordering::Relaxed));
        }
    }

    fn publish(&self, seq: u64, _start: Option<u64>) {
        if self.answers.load(Ordering::Relaxed) {
            let half = (seq % 2) as usize;
            for (o, &i) in self.outputs[half].iter().zip(&self.inputs[half]) {
                o.store(i.wrapping_neg(), Ordering::Relaxed);
            }
            self.ack.store(seq, Ordering::Release);
        }
    }

    fn ack(&self) -> u64 {
        self.ack.load(Ordering::Acquire)
    }

    fn count_late(&self) {
        self.late.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn the_bridge_block_is_realtime_safe() {
    let attached = Arc::new(AtomicBool::new(true));
    let answers = Arc::new(AtomicBool::new(true));
    let late = Arc::new(AtomicU64::new(0));
    let now = Arc::new(AtomicU64::new(0));
    let daw = Daw {
        attached: Arc::clone(&attached),
        answers: Arc::clone(&answers),
        inputs: [vec![0; FRAMES], vec![0; FRAMES]],
        outputs: [
            (0..FRAMES).map(|_| AtomicI32::new(0)).collect(),
            (0..FRAMES).map(|_| AtomicI32::new(0)).collect(),
        ],
        ack: AtomicU64::new(0),
        late: Arc::clone(&late),
    };
    let clock = Arc::clone(&now);
    let mut b = Bridge::with_spare(daw, 1)
        .with_deadline(FRAMES as u64, move || Some(clock.load(Ordering::Relaxed)))
        .with_concealment(CHANNELS, FRAMES);
    let input: Vec<f64> = (0..CHANNELS * FRAMES).map(|i| i as f64 / 1e3).collect();
    let mut output = vec![0.0f64; CHANNELS * FRAMES];
    // Each path in turn: answered, late (the fade out and the silence of a
    // gap), answered again (the fade in), detached, a discontinuity, and a
    // skipped schedule (stale answers).
    let mut start = 0u64;
    for n in 1..=150u64 {
        answers.store(!(40..50).contains(&n), Ordering::Relaxed);
        attached.store(!(70..75).contains(&n), Ordering::Relaxed);
        if n == 90 {
            b.discontinuity();
        }
        let step: u64 = if n == 110 { 10 } else { 1 };
        start += step * FRAMES as u64;
        now.store(start + 10 * FRAMES as u64, Ordering::Relaxed);
        let mut block = Block::new(FRAMES, &input, &mut output).at(start);
        b.process(&mut block);
    }
    assert!(late.load(Ordering::Relaxed) > 0, "the late path ran");
}

/// The device's processor behind the output watch in the test below.
struct Nothing;

impl Process for Nothing {
    fn process(&mut self, _: &mut Block<'_>) {}
}

/// The output watch on the pump's path (dasio#207): `Watched::process`
/// carries the same marker, so a zero/repeat channel and a test-tone
/// channel run every branch under the sanitizer: a zero break, the tone's
/// fits, its level windows, a click and its end, each block's figures, a
/// discontinuity and a jump in the media position.
#[test]
fn the_output_watch_is_realtime_safe() {
    const RATE: u32 = 96_000;
    const BLOCKS: usize = 600;
    let n = BLOCKS * FRAMES;
    let cfg = Config {
        name: "master".into(),
        block: FRAMES,
        rx: vec![(0, "rx".into())],
        tone: vec![(1, "tone".into())],
    };
    let set = Setup::new(cfg);
    let mut p = Watched::new(Nothing, Some(set.watch(RATE)));
    let w = std::f64::consts::TAU * 1_000.0 / f64::from(RATE);
    // RX 0: a loud sine with one block of digital silence (a zero break).
    let rx: Vec<f64> = (0..n)
        .map(|i| {
            if (3_200..3_200 + FRAMES).contains(&i) {
                0.0
            } else {
                0.5 * (w * i as f64).sin()
            }
        })
        .collect();
    // RX 1: a -20 dBFS tone, 24-bit, with a -40 dB click at 6 000.
    let lsb = 1.0 / 8_388_608.0;
    let mut tone: Vec<f64> = (0..n)
        .map(|i| (0.1 * (w * i as f64 + 0.3).sin() / lsb).round() * lsb)
        .collect();
    tone[6_000] += 1e-3;
    let mut input = [0.0f64; 2 * FRAMES];
    let mut output = [0.0f64; 2 * FRAMES];
    let mut start = 0u64;
    for b in 0..BLOCKS {
        let at = b * FRAMES;
        input[..FRAMES].copy_from_slice(&rx[at..at + FRAMES]);
        input[FRAMES..].copy_from_slice(&tone[at..at + FRAMES]);
        if b == 400 {
            p.discontinuity();
        }
        // A skipped schedule: the media position jumps a block.
        let blocks: u64 = if b == 500 { 2 } else { 1 };
        start += blocks * FRAMES as u64;
        let mut block = Block::new(FRAMES, &input, &mut output).at(start);
        p.process(&mut block);
    }
    let (mut tail, mut breaks) = (0, Vec::new());
    set.events.drain(&mut tail, &mut breaks);
    assert!(
        breaks.iter().any(|b| b.kind == Kind::Zero),
        "the zero break ran: {breaks:?}"
    );
    let (mut tail, mut tones) = (0, Vec::new());
    set.tones.events.drain(&mut tail, &mut tones);
    assert!(
        tones.iter().any(|b| b.kind == ToneKind::Click),
        "the tone's click ran: {tones:?}"
    );
    assert!(set.tones.take(0).checked > 0, "the tone was checked");
}

/// Allocates in a real-time context: a violation under the sanitizer.
#[cfg_attr(dasio_rtsan, sanitize(realtime = "nonblocking"))]
fn nonblocking_alloc(n: usize) -> usize {
    vec![0u8; n].len()
}

/// Run by `rtsan_detects_a_violation` in a child process; on its own it
/// does nothing.
#[test]
fn probe() {
    if std::env::var_os(PROBE).is_some() {
        assert_eq!(nonblocking_alloc(64), 64);
    }
}

#[test]
fn rtsan_detects_a_violation() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe", "--nocapture", "--test-threads", "1"])
        .env(PROBE, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if cfg!(dasio_rtsan) {
        assert!(!out.status.success(), "the allocation went unnoticed");
        assert!(stderr.contains("RealtimeSanitizer"), "{stderr}");
    } else {
        assert!(out.status.success(), "{stderr}");
    }
}
