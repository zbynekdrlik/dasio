use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// A stand-in DAW: `answers` says whether it returns a block; what it
/// returns is its inputs negated.
struct Fake {
    attached: bool,
    answers: bool,
    inputs: [Vec<i32>; 2],
    outputs: Mutex<[Vec<i32>; 2]>,
    ack: AtomicU64,
    late: AtomicU64,
    /// Each block published: its seq and media start.
    published: Mutex<Vec<(u64, Option<u64>)>>,
}

impl Fake {
    fn new(attached: bool, answers: bool, frames: usize) -> Self {
        Fake {
            attached,
            answers,
            inputs: [vec![0; frames], vec![0; frames]],
            outputs: Mutex::new([vec![0; frames], vec![0; frames]]),
            ack: AtomicU64::new(0),
            late: AtomicU64::new(0),
            published: Mutex::new(Vec::new()),
        }
    }
}

impl Link for Fake {
    fn attached(&self) -> bool {
        self.attached
    }
    fn input(&mut self, half: u32, _ch: usize) -> &mut [i32] {
        &mut self.inputs[half as usize]
    }
    fn read_output(&self, half: u32, _ch: usize, dst: &mut [f64]) {
        for (y, &s) in dst
            .iter_mut()
            .zip(&self.outputs.lock().unwrap()[half as usize])
        {
            *y = to_f64(s);
        }
    }
    fn publish(&self, seq: u64, start: Option<u64>) {
        self.published.lock().unwrap().push((seq, start));
        if self.answers {
            let half = (seq % 2) as usize;
            self.outputs.lock().unwrap()[half] = self.inputs[half].iter().map(|s| -s).collect();
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

fn run(bridge: &mut Bridge<Fake>, input: &[f64]) -> Vec<f64> {
    let mut output = vec![0.0; input.len()];
    let mut block = Block::new(input.len(), input, &mut output);
    bridge.process(&mut block);
    output
}

#[test]
fn a_block_s_answer_comes_back_with_the_next_block() {
    let mut b = Bridge::new(Fake::new(true, true, 4));
    let out = run(&mut b, &[0.5, -0.25, 0.0, 1.0 / 256.0]);
    assert_eq!(out, vec![0.0; 4], "nothing answered before the first block");
    assert_eq!(b.seq, 1);
    let out = run(&mut b, &[0.125; 4]);
    assert_eq!(out, vec![-0.5, 0.25, 0.0, -1.0 / 256.0], "block 1's answer");
    let out = run(&mut b, &[0.0; 4]);
    assert_eq!(
        out,
        vec![-0.125; 4],
        "block 2's answer, from the other half"
    );
    assert_eq!(b.link.late.load(Ordering::Relaxed), 0);
    assert_eq!(b.seq, 3);
}

#[test]
fn a_daw_that_does_not_answer_is_silence_and_late() {
    let mut b = Bridge::new(Fake::new(true, false, 4));
    assert_eq!(run(&mut b, &[0.5; 4]), vec![0.0; 4]);
    assert_eq!(run(&mut b, &[0.5; 4]), vec![0.0; 4]);
    assert_eq!(b.link.late.load(Ordering::Relaxed), 1, "block 1 unanswered");
    assert_eq!(b.seq, 2, "the pump never waits");
}

#[test]
fn no_daw_attached_is_silence_and_publishes_nothing() {
    let mut b = Bridge::new(Fake::new(false, true, 4));
    assert_eq!(run(&mut b, &[0.5; 4]), vec![0.0; 4]);
    assert_eq!(b.seq, 0);
}

#[test]
fn samples_convert_exactly_both_ways() {
    for s in [i32::MIN, -256, 0, 256, i32::MAX & !0xff] {
        assert_eq!(to_i32(to_f64(s)), s);
    }
    assert_eq!(to_i32(2.0), i32::MAX);
}

#[test]
fn the_first_block_reads_no_output_whatever_the_buffers_hold() {
    let mut b = Bridge::new(Fake::new(true, false, 4));
    // A previous session's block still in both halves.
    *b.link.outputs.lock().unwrap() = [vec![1 << 20; 4], vec![1 << 20; 4]];
    assert_eq!(run(&mut b, &[0.5; 4]), vec![0.0; 4]);
    assert_eq!(
        b.link.late.load(Ordering::Relaxed),
        0,
        "nothing was asked yet"
    );
}

/// The block at media time `start`.
fn run_at(bridge: &mut Bridge<Fake>, input: &[f64], start: u64) {
    let mut output = vec![0.0; input.len()];
    let mut block = Block::new(input.len(), input, &mut output).at(start);
    bridge.process(&mut block);
}

#[test]
fn each_block_is_published_with_its_media_time() {
    let mut b = Bridge::new(Fake::new(true, true, 4));
    run_at(&mut b, &[0.0; 4], 1_000);
    run_at(&mut b, &[0.0; 4], 1_004);
    // The pump skipped 8 blocks (a re-bootstrap): the seq stays one apart,
    // so the halves alternate, and the media time shows the skip.
    run_at(&mut b, &[0.0; 4], 1_040);
    run(&mut b, &[0.0; 4]);
    assert_eq!(
        *b.link.published.lock().unwrap(),
        vec![
            (1, Some(1_000)),
            (2, Some(1_004)),
            (3, Some(1_040)),
            (4, None)
        ]
    );
}
