//! The device loop's log of the output watch (dasio#207): the Windows glue
//! over the portable `crate::miss_watch`. Its start line, each break (the
//! zero and repeat breaks grouped over channels, each tone break on its
//! own) and every 10 s each tone channel's figures.

use tracing::{info, warn};

use crate::miss_watch::{self, Break, Setup, ToneBreak, tone};

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
}

/// A dB figure for the log: to a tenth.
fn tenth_db(ratio: f64) -> f64 {
    (tone::db(ratio) * 10.0).round() / 10.0
}

impl Reader {
    /// The breaks since the last tick (100 ms), logged; `now` reads the
    /// media position (samples at `rate`) for their age.
    pub(super) fn tick(&mut self, w: &Setup, now: impl Fn() -> Option<u64>, rate: u32) {
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
        if self.breaks.is_empty() && self.tone_breaks.is_empty() {
            return;
        }
        let now = now();
        for g in miss_watch::group(&self.breaks, w.cfg.block) {
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
    }
}

/// Each tone channel's figures since the last summary (every 10 s), logged.
pub(super) fn summary(w: &Setup, rate: u32) {
    for (i, (_, name)) in w.cfg.tone.iter().enumerate() {
        let s = w.tones.take(i);
        if s.checked == 0 {
            info!(
                watch = %w.cfg.name,
                channel = %name,
                "a watched test tone: nothing checked in the last 10 s (no tone, at or under \
                 -80 dBFS, outside 40 Hz to 0.45 of the rate, or not clean enough to fit) (dasio#207)"
            );
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
