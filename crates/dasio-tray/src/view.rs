//! What the tray shows (pure, tested everywhere): each instance's reading
//! (its service and its header, [`shm::Snapshot`]) over time to the icon's
//! colour, the tooltip and one menu line per instance.

use std::collections::{BTreeMap, VecDeque};

use shm::Snapshot;

/// Seconds the late count is taken over.
pub const LATE_WINDOW_S: u64 = 60;
/// A heartbeat unchanged this long: the device process is stuck.
pub const STUCK_S: u64 = 5;
/// Windows cuts a tray tooltip at 127 characters.
pub const TOOLTIP_MAX: usize = 127;

/// The instance's service, as the service manager reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    Running,
    Stopped,
    /// Starting, stopping, paused: the state's name.
    Other(String),
}

/// One instance at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub instance: u32,
    /// The name DAWs list it under (its ASIO entry), else `dasio <n>`.
    pub name: String,
    pub service: Service,
    /// Its header; `None` while no device made the mapping.
    pub snap: Option<Snapshot>,
}

/// The icon's colour, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Colour {
    /// A service is not running, or its device is stuck.
    Red,
    /// A late block in the last minute, or no DAW attached.
    Yellow,
    /// All running, a DAW on each, nothing late.
    Green,
    /// No instance installed.
    Grey,
}

/// What the tray shows now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub colour: Colour,
    pub tooltip: String,
    /// One per instance, by instance number.
    pub lines: Vec<String>,
}

/// One instance's past readings: its late counts over the window and when
/// its heartbeat last moved.
#[derive(Debug, Default)]
struct Past {
    late: VecDeque<(u64, u64)>,
    beat: Option<(u64, u64)>,
}

/// The readings over time (`now`: seconds of any steady clock).
#[derive(Debug, Default)]
pub struct Tracker {
    past: BTreeMap<u32, Past>,
}

impl Tracker {
    /// Takes `readings` at `now` and says what to show.
    pub fn update(&mut self, now: u64, readings: &[Reading]) -> View {
        self.past
            .retain(|n, _| readings.iter().any(|r| r.instance == *n));
        let mut colour = if readings.is_empty() {
            Colour::Grey
        } else {
            Colour::Green
        };
        let mut lines = Vec::new();
        let mut worst: Vec<String> = Vec::new();
        for r in readings {
            let past = self.past.entry(r.instance).or_default();
            let (c, state) = state(now, r, past);
            if c < colour {
                colour = c;
                worst.clear();
            }
            if c == colour && c != Colour::Green {
                worst.push(format!("{} {state}", r.name));
            }
            lines.push(line(r, &state, past));
        }
        let tooltip = match colour {
            Colour::Grey => "dasio: no instance installed".to_string(),
            Colour::Green => format!("dasio: {} running, all OK", readings.len()),
            _ => format!("dasio: {}", worst.join(", ")),
        };
        View {
            colour,
            tooltip: cut(tooltip, TOOLTIP_MAX),
            lines,
        }
    }
}

/// The instance's colour and state word; records its reading in `past`.
fn state(now: u64, r: &Reading, past: &mut Past) -> (Colour, String) {
    match (&r.service, &r.snap) {
        (Service::Stopped, _) => {
            *past = Past::default();
            (Colour::Red, "stopped".into())
        }
        (Service::Other(s), _) => {
            *past = Past::default();
            (Colour::Red, s.to_lowercase())
        }
        (Service::Running, None) => {
            *past = Past::default();
            (Colour::Red, "no device".into())
        }
        (Service::Running, Some(s)) => {
            // A restarted device starts its counts again: so does the past.
            if past.late.back().is_some_and(|&(_, l)| s.late < l) {
                *past = Past::default();
            }
            past.late.push_back((now, s.late));
            while past
                .late
                .front()
                .is_some_and(|&(t, _)| now.saturating_sub(t) > LATE_WINDOW_S)
            {
                past.late.pop_front();
            }
            match past.beat {
                Some((_, b)) if b == s.heartbeat_ms => {}
                _ => past.beat = Some((now, s.heartbeat_ms)),
            }
            let still = past.beat.map_or(0, |(t, _)| now.saturating_sub(t));
            if still >= STUCK_S {
                (Colour::Red, "stuck".into())
            } else if !s.attached {
                (Colour::Yellow, "no DAW".into())
            } else if late_in_window(past) > 0 {
                (Colour::Yellow, format!("{} late", late_in_window(past)))
            } else {
                (Colour::Green, "OK".into())
            }
        }
    }
}

/// Late blocks within the window (the newest count less the oldest kept).
fn late_in_window(past: &Past) -> u64 {
    match (past.late.front(), past.late.back()) {
        (Some(&(_, first)), Some(&(_, last))) => last - first,
        _ => 0,
    }
}

/// The instance's menu line.
fn line(r: &Reading, state: &str, past: &Past) -> String {
    let Some(s) = r.snap.filter(|_| r.service == Service::Running) else {
        return format!("{} (instance {}): {state}", r.name, r.instance);
    };
    let ms = |samples: u32| f64::from(samples) * 1000.0 / f64::from(s.rate.max(1));
    format!(
        "{} (instance {}): {state} · {} · {} @ {} kHz · {}/{} ch · in {:.2} / out {:.2} ms · late {}/min ({} total)",
        r.name,
        r.instance,
        if s.attached { "DAW attached" } else { "no DAW" },
        s.block,
        s.rate / 1000,
        s.rx,
        s.tx,
        ms(s.latency_in),
        ms(s.latency_out),
        late_in_window(past),
        s.late,
    )
}

/// `s` cut to `max` characters, an ellipsis at the end when cut.
fn cut(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The device name of an ASIO entry: the entry whose CLSID is instance
/// `n`'s (`entries`: each entry's name and CLSID text), else `dasio <n>`.
pub fn name_of(n: u32, clsid: &str, entries: &[(String, String)]) -> String {
    entries
        .iter()
        .find(|(_, c)| c.eq_ignore_ascii_case(clsid))
        .map_or_else(|| format!("dasio {n}"), |(name, _)| name.clone())
}

/// The instance number of a service name `dasio-<n>`.
pub fn instance_of_service(name: &str) -> Option<u32> {
    name.strip_prefix("dasio-")?
        .parse()
        .ok()
        .filter(|&n| (1..=16).contains(&n))
}

#[cfg(test)]
mod tests;
