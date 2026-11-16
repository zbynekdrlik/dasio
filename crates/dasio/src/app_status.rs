//! dasio's part of the device's status (dstack's `Media::set_app_status`),
//! which dtray, the tray all dstack devices share, shows: one `line` per
//! instance and a `state` for the icon's colour, plus the raw numbers for
//! its Copy status. Pure, so tested everywhere; the run loop feeds it.

use std::collections::VecDeque;

use dstack::media::tx::AppView;
use dstack_asio::bridge::{AgeReport, BlockAge};

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
    /// How audio TX goes out (dasio#125).
    pub tx: TxPath,
    /// The measured RX packet age at arrival (dasio#128): information,
    /// never advice; `None` before a packet was placed.
    pub rx: Option<crate::rx_arrival::Report>,
}

/// How the instance's audio TX goes out (dasio#125).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxPath {
    /// Through dasio's NDIS filter, with the filter's last 10 s window:
    /// frames that went to the UDP socket (no slot) and frames the NIC
    /// refused.
    Kernel { no_slot: u64, failed: u64 },
    /// On the UDP sockets: only the CI/test clock runs so.
    Udp,
}

impl Default for TxPath {
    /// Kernel TX, every frame taken: the only way a device on a real clock
    /// runs.
    fn default() -> Self {
        TxPath::Kernel {
            no_slot: 0,
            failed: 0,
        }
    }
}

impl TxPath {
    /// Frames missed the filter in the last window.
    fn bad(self) -> bool {
        matches!(self, TxPath::Kernel { no_slot, failed } if no_slot > 0 || failed > 0)
    }

    /// The tray line's TX part.
    fn suffix(self) -> String {
        match self {
            TxPath::Kernel {
                no_slot: 0,
                failed: 0,
            } => " · kernel TX".to_string(),
            TxPath::Kernel { no_slot, failed } => {
                format!(" · kernel TX: {no_slot} to the socket, {failed} failed (10 s)")
            }
            TxPath::Udp => " · UDP TX (test clock)".to_string(),
        }
    }

    /// The JSON fields: `kernel_tx`, `ktx_no_slot`, `ktx_failed`.
    fn json(self) -> String {
        match self {
            TxPath::Kernel { no_slot, failed } => {
                format!("\"kernel_tx\":true,\"ktx_no_slot\":{no_slot},\"ktx_failed\":{failed}")
            }
            TxPath::Udp => "\"kernel_tx\":false,\"ktx_no_slot\":0,\"ktx_failed\":0".to_string(),
        }
    }
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

/// The tray line's PC profile part: nothing when it is as it should be or
/// unchecked.
fn pc_suffix(problems: Option<u32>) -> String {
    match problems {
        Some(0) | None => String::new(),
        Some(1) => " · PC profile: 1 problem".to_string(),
        Some(n) => format!(" · PC profile: {n} problems"),
    }
}

fn ms(samples: u32, rate: u32) -> f64 {
    f64::from(samples) * 1000.0 / f64::from(rate.max(1))
}

/// The menu line dtray shows after the device's own.
pub fn line(f: &Facts) -> String {
    let pc = pc_suffix(f.pc_problems);
    let tx = f.tx.suffix();
    let rx = crate::rx_arrival::suffix(f.rx);
    format!(
        "{} · {} @ {} kHz{} · in {:.2} / out {:.2} ms · late {}/min ({} total){rx}{tx}{pc}",
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
/// last minute, with frames that missed the filter in its last window or
/// with a PC profile problem, else `ok`. (A device that stopped answers no status at
/// all: dtray shows that as an expected device not running.)
pub fn state(f: &Facts) -> &'static str {
    if !f.attached || f.late_per_min > 0 || f.tx.bad() || f.pc_problems.is_some_and(|n| n > 0) {
        "warn"
    } else {
        "ok"
    }
}

/// The JSON object for `set_app_status`: every number, `line` and `state`;
/// the RX arrival keys only once measured (dasio#128).
pub fn json(f: &Facts) -> String {
    let line = escaped(&line(f));
    format!(
        concat!(
            "{{\"line\":\"{}\",\"state\":\"{}\",\"daw_attached\":{},\"block\":{},",
            "\"sample_rate\":{},\"latency_in\":{},\"latency_out\":{},\"spare_blocks\":{},",
            "\"late_blocks\":{},\"late_per_min\":{},\"driver_missed\":{},\"stale\":{},",
            "\"waited\":{},\"heartbeat_ms\":{},\"pc_profile_problems\":{},{}{}}}"
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
        crate::rx_arrival::json(f.rx),
        f.tx.json(),
    )
}

/// What a bridge instance shows at one moment (dasio#106): its runner's
/// state and both players' counts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BridgeFacts {
    /// The ASIO driver's name.
    pub driver: String,
    /// The driver runs the bridge's side.
    pub running: bool,
    /// Why it does not (the runner's reason), and its stable code.
    pub reason: Option<String>,
    pub reason_code: Option<&'static str>,
    /// When the runner opens the driver again, s.
    pub retry_in_s: Option<f64>,
    /// The last open's rate (Hz) and buffer (frames).
    pub rate: u32,
    pub buffer_frames: u32,
    /// Blocks played as silence, Dante -> ASIO and ASIO -> Dante.
    pub to_asio_underruns: u64,
    pub from_asio_underruns: u64,
    /// Runs the runner closed (a reset, a rate change, a stall, a fault).
    pub resets: u64,
    /// The PC profile's problems, checked in process (dasio#120); `None`
    /// without a profile or before the first check.
    pub pc_problems: Option<u32>,
    /// How audio TX goes out (dasio#125).
    pub tx: TxPath,
    /// The from-ASIO figures (dstack #325, dasio#120); `None` while neither
    /// a block was taken nor a TX packet seen for 1-2 minutes.
    pub from_asio_age: Option<AgeReport>,
    /// The measured RX packet age at arrival (dasio#128): information,
    /// never advice; `None` before a packet was placed.
    pub rx: Option<crate::rx_arrival::Report>,
}

impl BridgeFacts {
    /// The facts of a bridge to `driver` from its runner's `status` and the
    /// two players' underruns.
    pub fn of(
        driver: &str,
        status: &dstack_asio::Status,
        to_asio_underruns: u64,
        from_asio_underruns: u64,
        pc_problems: Option<u32>,
        tx: TxPath,
        from_asio_age: Option<AgeReport>,
    ) -> Self {
        BridgeFacts {
            driver: driver.to_string(),
            running: status.state == dstack_asio::RunState::Running,
            reason: status.reason.as_ref().map(ToString::to_string),
            reason_code: status.reason.as_ref().map(dstack_asio::Reason::code),
            retry_in_s: status.retry_in_s,
            rate: status.rate,
            buffer_frames: status.buffer_frames,
            to_asio_underruns,
            from_asio_underruns,
            resets: status.resets,
            pc_problems,
            tx,
            from_asio_age,
            rx: None,
        }
    }
}

/// The bridge's from-ASIO keys for its status JSON (dstack #325,
/// dasio#120), each followed by a comma: each key only while its value is
/// known (never a 0 that reads as a fast driver or a young packet), all of
/// them left out without a report. `from_asio_block_age_*` is the modelled
/// age of the driver's blocks (a diagnostic); `from_asio_packet_age_us` the
/// TX packets' measured age past their stamps over the last minute, and
/// `from_asio_min_receiver_latency_us` that + the network.
fn age_json(age: Option<AgeReport>) -> String {
    let Some(a) = age else {
        return String::new();
    };
    [
        ("from_asio_block_age_us", a.block_age_us),
        ("from_asio_block_age_max_us", a.block_age_max_us),
        ("from_asio_packet_age_us", a.packet_age_us),
        (
            "from_asio_min_receiver_latency_us",
            a.min_receiver_latency_us,
        ),
    ]
    .into_iter()
    .filter_map(|(key, v)| v.map(|v| format!("\"{key}\":{v},")))
    .collect()
}

/// The bridge status loop's look at the TX side (dstack #325, dasio#120),
/// once a counts tick, never from the pump or the driver's callback (the
/// view locks the TX table): `view` from `TxShared::take_app_view` (this
/// device's only reader: a second one would take its peak), the device's
/// measured network p99 from `RxShared::network_p99_ns`. It records the TX
/// packets' age for the status and returns the warning `age` logged, at
/// most one an hour, when a receiver's late count rose.
pub fn from_asio_age_tick(
    age: &BlockAge,
    view: &AppView,
    network_p99_ns: Option<u64>,
) -> Option<String> {
    age.look(view, network_p99_ns)
}

/// The menu line dtray shows for a bridge instance.
pub fn bridge_line(f: &BridgeFacts) -> String {
    let now = if f.running {
        format!("running {} Hz / {}", f.rate, f.buffer_frames)
    } else {
        let why = f.reason.as_deref().unwrap_or("opening");
        match f.retry_in_s {
            Some(s) => format!("waiting ({why}), again in {s:.0} s"),
            None => format!("waiting ({why})"),
        }
    };
    format!(
        "bridge to \"{}\": {now} · underruns to ASIO {} / from ASIO {} · resets {}{}{}{}",
        f.driver,
        f.to_asio_underruns,
        f.from_asio_underruns,
        f.resets,
        crate::rx_arrival::suffix(f.rx),
        f.tx.suffix(),
        pc_suffix(f.pc_problems)
    )
}

/// The icon's state of a bridge instance: `ok` while the driver runs the
/// bridge (every frame through the filter, no PC profile problem), `warn`
/// otherwise.
pub fn bridge_state(f: &BridgeFacts) -> &'static str {
    if f.running && !f.tx.bad() && !f.pc_problems.is_some_and(|n| n > 0) {
        "ok"
    } else {
        "warn"
    }
}

/// `t` inside a JSON string (a driver name from the config may hold a
/// quote, a backslash or a control character).
fn escaped(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The JSON object for a bridge instance's `set_app_status`: `line`,
/// `state`, `kind` and every number (no DAW: `daw_attached` false); the
/// RX arrival and its ASIO pull wait keys only once measured (dasio#128).
pub fn bridge_json(f: &BridgeFacts) -> String {
    format!(
        concat!(
            "{{\"line\":\"{}\",\"state\":\"{}\",\"kind\":\"bridge\",\"daw_attached\":false,",
            "\"driver\":\"{}\",\"running\":{},\"reason\":\"{}\",\"rate\":{},",
            "\"buffer_frames\":{},\"to_asio_underruns\":{},\"from_asio_underruns\":{},",
            "\"resets\":{},\"pc_profile_problems\":{},{}{}{}{}}}"
        ),
        escaped(&bridge_line(f)),
        bridge_state(f),
        escaped(&f.driver),
        f.running,
        f.reason_code.unwrap_or(""),
        f.rate,
        f.buffer_frames,
        f.to_asio_underruns,
        f.from_asio_underruns,
        f.resets,
        f.pc_problems.map_or(-1, i64::from),
        age_json(f.from_asio_age),
        crate::rx_arrival::json(f.rx),
        crate::rx_arrival::pull_wait_json(f.rx),
        f.tx.json(),
    )
}

#[cfg(test)]
mod tests;
