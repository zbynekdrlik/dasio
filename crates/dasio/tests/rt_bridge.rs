// SPDX-License-Identifier: GPL-3.0-or-later

//! The bridge runs on the pump's real-time thread: none of its paths may
//! allocate (dasio#28 E13). Its own test binary, so this allocator counts
//! allocations inside `assert_no_alloc` without touching the other tests.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use assert_no_alloc::{AllocDisabler, assert_no_alloc, reset_violation_count, violation_count};
use dasio::bridge::{Bridge, Link, to_f64};
use dstack_audio_io::{Block, Process};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const FRAMES: usize = 32;
const CHANNELS: usize = 2;

/// A stand-in DAW whose buffers are all allocated up front: it answers
/// each block with its inputs negated while `answers` is set.
struct Daw {
    attached: Arc<AtomicBool>,
    answers: Arc<AtomicBool>,
    inputs: [Vec<i32>; 2],
    outputs: Mutex<[Vec<i32>; 2]>,
    ack: AtomicU64,
    late: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
}

impl Link for Daw {
    fn attached(&self) -> bool {
        self.attached.load(Ordering::Relaxed)
    }

    fn input(&mut self, half: u32, _ch: usize) -> &mut [i32] {
        &mut self.inputs[half as usize]
    }

    fn read_output(&self, half: u32, _ch: usize, dst: &mut [f64]) {
        let outputs = self.outputs.lock().unwrap();
        for (y, &s) in dst.iter_mut().zip(&outputs[half as usize]) {
            *y = to_f64(s);
        }
    }

    fn publish(&self, seq: u64, _start: Option<u64>) {
        self.published.fetch_add(1, Ordering::Relaxed);
        if self.answers.load(Ordering::Relaxed) {
            let half = (seq % 2) as usize;
            let mut outputs = self.outputs.lock().unwrap();
            for (o, &i) in outputs[half].iter_mut().zip(&self.inputs[half]) {
                *o = i.wrapping_neg();
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

/// What the test reads and steers from outside the bridge.
struct Knobs {
    attached: Arc<AtomicBool>,
    answers: Arc<AtomicBool>,
    late: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
    /// The media clock the bridge's deadline reads, in samples.
    now: Arc<AtomicU64>,
}

/// A bridge as the service builds it (spare blocks, concealment, a
/// deadline on the media clock) around a stand-in DAW.
fn bridge(spare: u64) -> (Bridge<Daw>, Knobs) {
    let knobs = Knobs {
        attached: Arc::new(AtomicBool::new(true)),
        answers: Arc::new(AtomicBool::new(true)),
        late: Arc::new(AtomicU64::new(0)),
        published: Arc::new(AtomicU64::new(0)),
        now: Arc::new(AtomicU64::new(0)),
    };
    let daw = Daw {
        attached: Arc::clone(&knobs.attached),
        answers: Arc::clone(&knobs.answers),
        inputs: [vec![0; FRAMES], vec![0; FRAMES]],
        outputs: Mutex::new([vec![0; FRAMES], vec![0; FRAMES]]),
        ack: AtomicU64::new(0),
        late: Arc::clone(&knobs.late),
        published: Arc::clone(&knobs.published),
    };
    let now = Arc::clone(&knobs.now);
    let b = Bridge::with_spare(daw, spare)
        .with_deadline(FRAMES as u64, move || Some(now.load(Ordering::Relaxed)))
        .with_concealment(CHANNELS, FRAMES);
    (b, knobs)
}

/// `blocks` blocks through `b`, each path in turn: answered, late (the
/// fade out and the silence of a gap), answered again (the fade in),
/// detached, a discontinuity, and blocks after a skipped schedule (stale
/// answers). The media clock is always past the deadline, so a missing
/// answer is late at once.
fn run(b: &mut Bridge<Daw>, k: &Knobs, input: &[f64], output: &mut [f64], blocks: u64) {
    let mut start = 0u64;
    for n in 1..=blocks {
        k.answers.store(!(40..50).contains(&n), Ordering::Relaxed);
        k.attached.store(!(70..75).contains(&n), Ordering::Relaxed);
        if n == 90 {
            b.discontinuity();
        }
        // A pump that re-bootstrapped: the schedule jumps by 10 blocks.
        let step: u64 = if n == 110 { 10 } else { 1 };
        start += step * FRAMES as u64;
        k.now.store(start + 10 * FRAMES as u64, Ordering::Relaxed);
        let mut block = Block::new(FRAMES, input, &mut *output).at(start);
        b.process(&mut block);
    }
}

#[test]
fn the_bridge_never_allocates_in_its_block_paths() {
    // The check itself catches an allocation (the counter is per binary,
    // so this runs here, not as a test of its own beside the others).
    reset_violation_count();
    let v = assert_no_alloc(|| vec![1u8; 4]);
    assert!(violation_count() > 0, "the allocation check is live");
    assert_eq!(v.len(), 4);

    let input: Vec<f64> = (0..CHANNELS * FRAMES)
        .map(|i| (i as f64 / 100.0).sin() * 0.5)
        .collect();
    let mut output = vec![0.0; CHANNELS * FRAMES];
    for spare in [0, 1] {
        let (mut b, k) = bridge(spare);
        let stale = b.stale_counter();
        reset_violation_count();
        assert_no_alloc(|| run(&mut b, &k, &input, &mut output, 150));
        assert_eq!(violation_count(), 0, "the bridge allocated (spare {spare})");
        // Every path ran: blocks went to the DAW, some were late, some
        // answers were stale after the skipped schedule.
        assert!(k.published.load(Ordering::Relaxed) > 100, "spare {spare}");
        assert!(
            k.late.load(Ordering::Relaxed) > 0,
            "late blocks (spare {spare})"
        );
        assert!(
            stale.load(Ordering::Relaxed) > 0,
            "stale answers (spare {spare})"
        );
    }
}
