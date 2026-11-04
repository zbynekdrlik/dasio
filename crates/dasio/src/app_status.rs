//! dasio's part of the device's status (dstack's `Media::set_app_status`),
//! which dtray, the tray all dstack devices share, shows: one `line` per
//! instance and a `state` for the icon's colour, plus the raw numbers for
//! its Copy status. Pure, so tested everywhere; the run loop feeds it.

use std::collections::VecDeque;

/// Seconds the late count per minute is taken over.
pub const LATE_WINDOW_S: u64 = 60;

/// What the instance shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    pub attached: bool,
    pub block: u32,
    pub sample_rate: u32,
    pub latency_in: u32,
    pub latency_out: u32,
    pub spare_blocks: u32,
    pub late_blocks: u64,
    pub driver_missed: u64,
    pub stale: u64,
    pub waited: u64,
    pub heartbeat_ms: u64,
    /// Late blocks in the last [`LATE_WINDOW_S`] seconds ([`LateWindow`]).
    pub late_per_min: u64,
    /// The PC profile's problems (dasio#5); `None` without a profile.
    pub pc_problems: Option<u32>,
}

/// The late counter over the last minute.
#[derive(Debug, Default)]
pub struct LateWindow {
    seen: VecDeque<(u64, u64)>,
}

impl LateWindow {
    /// Takes the counter `late` at `now_s` (seconds of any steady clock);
    /// the late blocks within the window. A counter that went down (a
    /// restarted device) starts the window again.
    pub fn take(&mut self, now_s: u64, late: u64) -> u64 {
        if self.seen.back().is_some_and(|&(_, l)| late < l) {
            self.seen.clear();
        }
        self.seen.push_back((now_s, late));
        while self
            .seen
            .front()
            .is_some_and(|&(t, _)| now_s.saturating_sub(t) > LATE_WINDOW_S)
        {
            self.seen.pop_front();
        }
        late - self.seen.front().map_or(late, |&(_, l)| l)
    }
}

fn ms(samples: u32, rate: u32) -> f64 {
    f64::from(samples) * 1000.0 / f64::from(rate.max(1))
}

/// The menu line dtray shows after the device's own.
pub fn line(f: &Facts) -> String {
    let pc = match f.pc_problems {
        Some(0) | None => String::new(),
        Some(1) => " · PC profile: 1 problem".to_string(),
        Some(n) => format!(" · PC profile: {n} problems"),
    };
    format!(
        "{} · {} @ {} kHz{} · in {:.2} / out {:.2} ms · late {}/min ({} total){pc}",
        if f.attached { "DAW attached" } else { "no DAW" },
        f.block,
        f.sample_rate / 1000,
        if f.spare_blocks > 0 { " + spare" } else { "" },
        ms(f.latency_in, f.sample_rate),
        ms(f.latency_out, f.sample_rate),
        f.late_per_min,
        f.late_blocks,
    )
}

/// The icon's state: `warn` without a DAW, with a late block in the
/// last minute or with a PC profile problem, else `ok`. (A device that stopped answers no status at
/// all: dtray shows that as an expected device not running.)
pub fn state(f: &Facts) -> &'static str {
    if !f.attached || f.late_per_min > 0 || f.pc_problems.is_some_and(|n| n > 0) {
        "warn"
    } else {
        "ok"
    }
}

/// The JSON object for `set_app_status`: every number, `line` and `state`.
pub fn json(f: &Facts) -> String {
    let line = line(f).replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        concat!(
            "{{\"line\":\"{}\",\"state\":\"{}\",\"daw_attached\":{},\"block\":{},",
            "\"sample_rate\":{},\"latency_in\":{},\"latency_out\":{},\"spare_blocks\":{},",
            "\"late_blocks\":{},\"late_per_min\":{},\"driver_missed\":{},\"stale\":{},",
            "\"waited\":{},\"heartbeat_ms\":{},\"pc_profile_problems\":{}}}"
        ),
        line,
        state(f),
        f.attached,
        f.block,
        f.sample_rate,
        f.latency_in,
        f.latency_out,
        f.spare_blocks,
        f.late_blocks,
        f.late_per_min,
        f.driver_missed,
        f.stale,
        f.waited,
        f.heartbeat_ms,
        f.pc_problems.map_or(-1, i64::from),
    )
}

#[cfg(test)]
mod tests;
