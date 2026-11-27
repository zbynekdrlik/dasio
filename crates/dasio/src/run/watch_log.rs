//! The device loop's log of the output watch (dasio#207): the Windows glue
//! over the portable `crate::miss_watch`. Its start line, each break (the
//! zero and repeat breaks grouped over channels, each tone break on its
//! own, the program splices and the program loops grouped over channels)
//! and every 10 s each tone and program channel's figures.

use tracing::{info, warn};

use crate::miss_watch::program::{self, Tracker};
use crate::miss_watch::silence::Silence;
use crate::miss_watch::stall::{Hold, Stall};
use crate::miss_watch::{self, Break, Loop, ProgramJump, Setup, ToneBreak, looped, tone};

/// The watch of the instance folder `dir` (its watch.toml) against the
/// device's RX channel names, its start logged. A config that does not read
/// leaves the watch off and the device running.
pub(super) fn start(dir: &std::path::Path, device_rx: &[String]) -> Option<Setup> {
    match miss_watch::setup(dir, device_rx) {
        Ok(Some(c)) => {
            info!(
                watch = %c.name,
                block = c.block,
                rx = %names(&c.rx),
                tone = %names(&c.tone),
                program = %names(&c.program),
                "watching received outputs for an ASIO host's missed buffers (dasio#207)"
            );
            Some(Setup::new(c))
        }
        Ok(None) => None,
        Err(e) => {
            warn!(err = %e, "the output watch is off: its config does not read (dasio#207)");
            None
        }
    }
}

fn names(list: &[(usize, String)]) -> String {
    list.iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The loop's side of the watch's rings: where it read to, and its buffers.
#[derive(Default)]
pub(super) struct Reader {
    tail: u64,
    breaks: Vec<Break>,
    tone_tail: u64,
    tone_breaks: Vec<ToneBreak>,
    program_tail: u64,
    program_jumps: Vec<ProgramJump>,
    program_lines: Tracker,
    loop_tail: u64,
    loops: Vec<Loop>,
    tone_silence: Silence,
    program_silence: Silence,
    /// Which RX sources lost packets lately (the device's own counters).
    stall: Stall,
    /// The zero and repeat breaks and the program jumps, judged two ticks
    /// after they were drained (an RX stall's gaps are confirmed late).
    held_breaks: Hold<Break>,
    held_jumps: Hold<ProgramJump>,
}

/// A dB figure for the log: to a tenth.
fn tenth_db(ratio: f64) -> f64 {
    (tone::db(ratio) * 10.0).round() / 10.0
}

impl Reader {
    /// The breaks since the last tick (100 ms), logged; `now` reads the
    /// media position (samples at `rate`) for their age. `rx_gaps` is each
    /// RX flow slot's cumulative count of lost-packet gaps: the zero and
    /// repeat breaks and the program jumps are held two ticks and logged as
    /// a host's miss and a jump, or, with several sources losing packets around
    /// them, as an RX stall's cut.
    pub(super) fn tick(
        &mut self,
        w: &Setup,
        now: impl Fn() -> Option<u64>,
        rate: u32,
        rx_gaps: &[u64],
        rx_flows: &[u64],
    ) {
        self.breaks.clear();
        let lost = w.events.drain(&mut self.tail, &mut self.breaks);
        if lost > 0 {
            warn!(
                lost,
                "the output watch lost breaks: more than its ring holds in 100 ms"
            );
        }
        self.tone_breaks.clear();
        let lost = w
            .tones
            .events
            .drain(&mut self.tone_tail, &mut self.tone_breaks);
        if lost > 0 {
            warn!(
                lost,
                "the tone check lost breaks: more than its ring holds in 100 ms (dasio#207)"
            );
        }
        self.program_jumps.clear();
        let lost = w
            .programs
            .events
            .drain(&mut self.program_tail, &mut self.program_jumps);
        if lost > 0 {
            warn!(
                lost,
                "the program check lost events: more than its ring holds in 100 ms (dasio#207)"
            );
        }
        self.loops.clear();
        let lost = w.programs.loops.drain(&mut self.loop_tail, &mut self.loops);
        if lost > 0 {
            warn!(
                lost,
                "the loop check lost events: more than its ring holds in 100 ms (dasio#207)"
            );
        }
        self.stall.note(rx_gaps, rx_flows);
        let due_breaks = self.held_breaks.tick(std::mem::take(&mut self.breaks));
        let due_jumps = self
            .held_jumps
            .tick(std::mem::take(&mut self.program_jumps));
        if due_breaks.is_empty()
            && due_jumps.is_empty()
            && self.tone_breaks.is_empty()
            && self.loops.is_empty()
        {
            return;
        }
        let now = now();
        self.log_breaks(w, &due_breaks, now, rate);
        for b in &self.tone_breaks {
            warn!(
                watch = %w.cfg.name,
                channel = %w.cfg.tone.get(b.channel).map_or("?", |(_, n)| n.as_str()),
                kind = b.kind.name(),
                at = b.at,
                samples = b.len,
                db = tenth_db(b.rel),
                ago_ms = now.map(|n| miss_watch::age_ms(b.at, n, rate)),
                "a watched test tone broke: click = samples off the sine (db: the worst residual \
                 against the tone), level = its level stepped (db: the step) (dasio#207)"
            );
        }
        self.log_jumps(w, &due_jumps, now, rate);
        for l in looped::lines(&self.loops) {
            warn!(
                watch = %w.cfg.name,
                channels = %miss_watch::program_names(&l.channels, &w.cfg),
                at = l.at,
                lag = l.lag,
                samples = l.len,
                ms = looped::ms(l.len, rate),
                peak_dbfs = tenth_db(l.peak),
                grid = program::grid(l.at, w.cfg.block),
                ago_ms = now.map(|n| miss_watch::age_ms(l.at, n, rate)),
                "a watched program output looped: the same samples came again `lag` samples \
                 later, over `samples` samples (a device replaying its buffer while its host \
                 does not serve it); peak_dbfs = the loudest repeated sample (dasio#207)"
            );
        }
    }

    /// The zero and repeat breaks due now, as a host's miss or, in an RX
    /// stall, as its cut (a line the event trigger does not match).
    fn log_breaks(&self, w: &Setup, due: &[Break], now: Option<u64>, rate: u32) {
        let stalled = self.stall.stalled();
        for g in miss_watch::group(due, w.cfg.block) {
            if stalled {
                warn!(
                    watch = %w.cfg.name,
                    kind = g.kind.name(),
                    samples = g.len,
                    ago_ms = now.map(|n| miss_watch::age_ms(g.at, n, rate)),
                    channels = %miss_watch::channel_names(&g, &w.cfg),
                    broke = g.channels.len(),
                    of = w.cfg.rx.len(),
                    sources = self.stall.sources(),
                    "a watched output was cut while RX packets were missing from several \
                     sources: an RX stall, not an ASIO host's missed buffer (dasio#207)"
                );
                continue;
            }
            warn!(
                watch = %w.cfg.name,
                kind = g.kind.name(),
                samples = g.len,
                ago_ms = now.map(|n| miss_watch::age_ms(g.at, n, rate)),
                channels = %miss_watch::channel_names(&g, &w.cfg),
                broke = g.channels.len(),
                of = w.cfg.rx.len(),
                "a watched output broke like an ASIO host's missed buffer (dasio#207)"
            );
        }
    }

    /// The program jumps due now, as a jump or, in an RX stall, as its cut.
    fn log_jumps(&mut self, w: &Setup, jumps: &[ProgramJump], now: Option<u64>, rate: u32) {
        let stalled = self.stall.stalled();
        for l in self.program_lines.lines(jumps, w.cfg.block) {
            if stalled {
                warn!(
                    watch = %w.cfg.name,
                    channels = %miss_watch::program_names(&l.channels, &w.cfg),
                    at = l.at,
                    db = tenth_db(l.ratio),
                    peak_dbfs = program::peak_dbfs(l.steps),
                    grid = l.grid,
                    ago_ms = now.map(|n| miss_watch::age_ms(l.at, n, rate)),
                    sources = self.stall.sources(),
                    "a watched program audio step came while RX packets were missing from several \
                     sources: an RX stall's cut, not an ASIO host's missed buffer (dasio#207)"
                );
                continue;
            }
            warn!(
                watch = %w.cfg.name,
                channels = %miss_watch::program_names(&l.channels, &w.cfg),
                at = l.at,
                db = tenth_db(l.ratio),
                peak_dbfs = program::peak_dbfs(l.steps),
                grid = l.grid,
                since_prev = l.since_prev,
                rem = l.rem,
                ago_ms = now.map(|n| miss_watch::age_ms(l.at, n, rate)),
                "a watched program output jumped: a step in the audio, db = its peak over the \
                 quiet around it, peak_dbfs = the peak itself (under about -50 is a step nobody \
                 hears), grid = at modulo the block (the host's misses share one value), rem = the distance to the channel's previous jump modulo the \
                 host block (0 = a whole number of blocks, like the host's misses) (dasio#207)"
            );
        }
    }

    /// What is still held, logged now (the device loop ends): judged by the
    /// stall state at this last tick.
    pub(super) fn flush(&mut self, w: &Setup, now: impl Fn() -> Option<u64>, rate: u32) {
        let breaks = self.held_breaks.flush();
        let jumps = self.held_jumps.flush();
        if breaks.is_empty() && jumps.is_empty() {
            return;
        }
        let now = now();
        self.log_breaks(w, &breaks, now, rate);
        self.log_jumps(w, &jumps, now, rate);
    }
}

/// Each tone and program channel's figures since the last summary (every
/// 10 s), logged.
pub(super) fn summary(r: &mut Reader, w: &Setup, rate: u32) {
    program_summary(&mut r.program_silence, w);
    for (i, (_, name)) in w.cfg.tone.iter().enumerate() {
        let s = w.tones.take(i);
        let silent = r.tone_silence.note(i, s.checked > 0);
        if s.checked == 0 {
            // Logged at the first silent window and then every 10 min.
            if let Some(silent_s) = silent {
                info!(
                    watch = %w.cfg.name,
                    channel = %name,
                    silent_s,
                    "a watched test tone: nothing checked in the last 10 s (no tone, at or under \
                     -80 dBFS, outside 40 Hz to 0.45 of the rate, or not clean enough to fit) \
                     (dasio#207)"
                );
            }
            continue;
        }
        // No level window ended (breaks faster than one): no amplitude.
        let levels = s.amp_max > 0.0;
        let hz = (tone::hz(s.c, rate) * 10.0).round() / 10.0;
        info!(
            watch = %w.cfg.name,
            channel = %name,
            checked = s.checked,
            breaks = s.breaks,
            worst_db = tenth_db(s.worst),
            amp_min_dbfs = levels.then(|| tenth_db(s.amp_min)),
            amp_max_dbfs = levels.then(|| tenth_db(s.amp_max)),
            hz,
            "a watched test tone over the last 10 s (dasio#207)"
        );
        let jump = tone::block_jump(s.c, w.cfg.block);
        if tone::blind(jump) {
            warn!(
                watch = %w.cfg.name,
                channel = %name,
                hz,
                block = w.cfg.block,
                jump_db = tenth_db(jump),
                "a watched test tone is blind to a repeated or skipped host block: the block is \
                 a whole number of its periods; give the tone another frequency, e.g. 440 Hz \
                 (dasio#207)"
            );
        }
    }
}

/// Each program channel's figures since the last summary (every 10 s).
fn program_summary(silence: &mut Silence, w: &Setup) {
    for (i, (_, name)) in w.cfg.program.iter().enumerate() {
        let s = w.programs.take(i);
        let silent = silence.note(i, s.checked > 0);
        if s.checked == 0 {
            if let Some(silent_s) = silent {
                info!(
                    watch = %w.cfg.name,
                    channel = %name,
                    silent_s,
                    "a watched program output: nothing checked in the last 10 s (digital \
                     silence) (dasio#207)"
                );
            }
            continue;
        }
        info!(
            watch = %w.cfg.name,
            channel = %name,
            checked = s.checked,
            jumps = s.jumps,
            worst_db = (s.worst > 0.0).then(|| tenth_db(s.worst)),
            floor_dbfs = (program::floor_dbfs(&s) * 10.0).round() / 10.0,
            "a watched program output over the last 10 s (dasio#207)"
        );
    }
}
