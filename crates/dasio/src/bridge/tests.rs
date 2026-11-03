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

/// A DAW that answers on its own time: the bridge's media clock is the test
/// clock `t` (one sample per read), and the DAW returns block `answers.0`
/// once `t` reaches `answers.1`. Its outputs are always 0.5.
struct Slow {
    t: Arc<AtomicU64>,
    ack: Arc<AtomicU64>,
    /// Each block published: its seq and the clock then.
    published: Mutex<Vec<(u64, u64)>>,
    late: AtomicU64,
}

impl Link for Slow {
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
        let t = self.t.load(Ordering::Acquire);
        self.published.lock().unwrap().push((seq, t));
    }
    fn ack(&self) -> u64 {
        self.ack.load(Ordering::Acquire)
    }
    fn count_late(&self) {
        self.late.fetch_add(1, Ordering::Relaxed);
    }
}

/// A bridge on a `Slow` DAW whose answer to block `seq` comes at clock
/// `at`; the bridge may wait until `offset` samples after a block's start.
/// Returns the bridge, the clock and the count of clock reads.
fn slow(
    spare: u64,
    offset: u64,
    seq: u64,
    at: u64,
) -> (Bridge<Slow>, Arc<AtomicU64>, Arc<AtomicU64>) {
    let t = Arc::new(AtomicU64::new(0));
    let ack = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let link = Slow {
        t: Arc::clone(&t),
        ack: Arc::clone(&ack),
        published: Mutex::new(Vec::new()),
        late: AtomicU64::new(0),
    };
    let (clock, count) = (Arc::clone(&t), Arc::clone(&reads));
    let now = move || {
        count.fetch_add(1, Ordering::Relaxed);
        let now = clock.fetch_add(1, Ordering::AcqRel);
        if now >= at {
            ack.fetch_max(seq, Ordering::AcqRel);
        }
        Some(now)
    };
    let b = Bridge::with_spare(link, spare).with_deadline(offset, now);
    (b, t, reads)
}

fn slow_block(bridge: &mut Bridge<Slow>, start: u64) -> Vec<f64> {
    let input = [0.0; 4];
    let mut output = vec![0.0; 4];
    let mut block = Block::new(4, &input, &mut output).at(start);
    bridge.process(&mut block);
    output
}

#[test]
fn an_answer_that_comes_within_the_work_margin_is_played() {
    // Block 1 starts at 0; block 2 at 4 is due at once but the DAW answers
    // block 1 only at clock 9: inside the margin (block 2's start + 10).
    let (mut b, t, reads) = slow(0, 10, 1, 9);
    let waited = b.waited_counter();
    slow_block(&mut b, 0);
    t.store(5, Ordering::Release);
    let out = slow_block(&mut b, 4);
    assert_eq!(out, vec![0.5; 4], "the answer that came in time is played");
    assert_eq!(b.link.late.load(Ordering::Relaxed), 0);
    assert_eq!(waited.load(Ordering::Relaxed), 1, "saved by the wait");
    // The wait ends with the answer (clock reads 5..=9), not at the deadline.
    assert_eq!(reads.load(Ordering::Relaxed), 5);
    // Without a spare block the DAW has block 2 while the bridge waits.
    let published = b.link.published.lock().unwrap().clone();
    assert_eq!(published.len(), 2);
    assert_eq!(published[1].0, 2);
    assert!(
        published[1].1 < 9,
        "block 2 went out before the wait: {published:?}"
    );
}

#[test]
fn the_wait_ends_at_the_deadline() {
    // The DAW never answers in time: the bridge stops at block 2's start +
    // 10 (clock 14) and the block is late.
    let (mut b, t, reads) = slow(0, 10, 1, 1_000);
    let waited = b.waited_counter();
    slow_block(&mut b, 0);
    t.store(5, Ordering::Release);
    let out = slow_block(&mut b, 4);
    assert_eq!(out, vec![0.0; 4], "no answer: silence");
    assert_eq!(b.link.late.load(Ordering::Relaxed), 1);
    assert_eq!(waited.load(Ordering::Relaxed), 0, "late, not saved");
    // Clock reads 5..=14: the read at the deadline (14) ends the wait.
    assert_eq!(
        t.load(Ordering::Acquire),
        15,
        "waited until the deadline, no longer"
    );
    assert_eq!(reads.load(Ordering::Relaxed), 10, "a bounded wait");
}

#[test]
fn with_a_spare_block_the_outputs_are_read_before_the_next_block_goes_out() {
    // Spare 1: block 3's outputs half is block 1's; the bridge reads block
    // 1's answer (at clock 9) before it hands block 3 to the DAW.
    let (mut b, t, _) = slow(1, 10, 1, 9);
    slow_block(&mut b, 0);
    slow_block(&mut b, 4);
    t.store(5, Ordering::Release);
    let out = slow_block(&mut b, 8);
    assert_eq!(out, vec![0.5; 4], "block 1's answer, two blocks on");
    assert_eq!(b.link.late.load(Ordering::Relaxed), 0);
    let published = b.link.published.lock().unwrap().clone();
    assert_eq!(published[2].0, 3);
    assert!(
        published[2].1 >= 9,
        "block 3 went out after the read: {published:?}"
    );
}

#[test]
fn an_answer_already_in_needs_no_wait() {
    // The DAW answered block 1 at once: no clock read, nothing counted.
    let (mut b, t, reads) = slow(0, 10, 1, 0);
    let waited = b.waited_counter();
    slow_block(&mut b, 0);
    b.link.ack.store(1, Ordering::Release);
    t.store(5, Ordering::Release);
    let out = slow_block(&mut b, 4);
    assert_eq!(out, vec![0.5; 4]);
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    assert_eq!(waited.load(Ordering::Relaxed), 0);
}
