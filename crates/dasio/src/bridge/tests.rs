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

/// The stand-in DAW answers block `seq` now: its inputs negated.
fn answer(b: &Bridge<Fake>, seq: u64) {
    let half = (seq % 2) as usize;
    b.link.outputs.lock().unwrap()[half] = b.link.inputs[half].iter().map(|s| -s).collect();
    b.link.ack.store(seq, Ordering::Release);
}

#[test]
fn a_spare_block_gives_the_daw_a_second_period() {
    // FOH PC, band Ableton at block 256: most blocks back within 1 ms, some
    // after 2.7 to 4.7 ms (dasio#1). With one spare block an answer one
    // period late still plays, one block later.
    let mut b = Bridge::with_spare(Fake::new(true, false, 2), 1);
    run(&mut b, &[0.25; 2]);
    let out = run(&mut b, &[0.5; 2]);
    assert_eq!(out, vec![0.0; 2], "nothing due yet");
    assert_eq!(
        b.link.late.load(Ordering::Relaxed),
        0,
        "block 1 still has time"
    );
    answer(&b, 1);
    let out = run(&mut b, &[0.0; 2]);
    assert_eq!(out, vec![-0.25; 2], "block 1's answer, one block later");
    assert_eq!(b.link.late.load(Ordering::Relaxed), 0);
    // Block 2 not back two periods on: late, silence.
    let out = run(&mut b, &[0.0; 2]);
    assert_eq!(out, vec![0.0; 2]);
    assert_eq!(
        b.link.late.load(Ordering::Relaxed),
        1,
        "block 2 missed both periods"
    );
}

#[test]
fn without_a_spare_block_an_answer_one_period_late_is_lost() {
    let mut b = Bridge::with_spare(Fake::new(true, false, 2), 0);
    run(&mut b, &[0.25; 2]);
    let out = run(&mut b, &[0.5; 2]);
    assert_eq!(out, vec![0.0; 2]);
    assert_eq!(b.link.late.load(Ordering::Relaxed), 1);
}

fn played_at(bridge: &mut Bridge<Fake>, input: &[f64], start: u64) -> Vec<f64> {
    let mut output = vec![0.0; input.len()];
    let mut block = Block::new(input.len(), input, &mut output).at(start);
    bridge.process(&mut block);
    output
}

#[test]
fn an_answer_whose_slot_has_passed_is_dropped() {
    // The pump re-bootstrapped and skipped 8 blocks between block 1 and the
    // next one it ran: block 1's answer belongs at media time 2, not 18
    // (CI 41567658657: one impulse came back 7 blocks late, outside the
    // latency the driver reports). Dropped, as a sound card drops it.
    let mut b = Bridge::new(Fake::new(true, true, 2));
    let stale = b.stale_counter();
    played_at(&mut b, &[0.25; 2], 0);
    let out = played_at(&mut b, &[0.5; 2], 2 + 8 * 2);
    assert_eq!(
        out,
        vec![0.0; 2],
        "block 1's answer is not played 8 blocks late"
    );
    assert_eq!(
        b.link.late.load(Ordering::Relaxed),
        0,
        "not the DAW's lateness"
    );
    assert_eq!(stale.load(Ordering::Relaxed), 1);
    let out = played_at(&mut b, &[0.0; 2], 2 + 9 * 2);
    assert_eq!(out, vec![-0.5; 2], "the next answer plays in its own slot");
    assert_eq!(stale.load(Ordering::Relaxed), 1);
}

#[test]
fn with_a_spare_block_the_slot_is_two_blocks_on() {
    let mut b = Bridge::with_spare(Fake::new(true, true, 2), 1);
    let stale = b.stale_counter();
    played_at(&mut b, &[0.25; 2], 0);
    played_at(&mut b, &[0.5; 2], 2);
    // Block 1 (start 0) plays at 4.
    let out = played_at(&mut b, &[0.0; 2], 4);
    assert_eq!(out, vec![-0.25; 2]);
    // Block 2 (start 2) belongs at 6; the pump jumped to 10.
    let out = played_at(&mut b, &[0.0; 2], 10);
    assert_eq!(out, vec![0.0; 2]);
    assert_eq!(stale.load(Ordering::Relaxed), 1);
}
