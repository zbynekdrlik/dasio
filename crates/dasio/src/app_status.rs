//! dasio's part of the device's status (dstack's `Media::set_app_status`),
//! which dtray, the tray all dstack devices share, shows: one `line` per
//! instance and a `state` for the icon's colour, plus the raw numbers for
//! its Copy status. Pure, so tested everywhere; the run loop feeds it.

use std::collections::VecDeque;

use dstack::media::tx::{AppView, TxFillView};
use dstack_asio::bridge::{AgeReport, BlockAge, UnderrunCounts, UnderrunWatch};
use dstack_asio::state::{LATE_BIN_KEYS, LATE_BINS};

use crate::startup::Part;

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
    /// The start-up window (dasio#28): `late_blocks`, `driver_missed`,
    /// `stale` and `waited` above are its counts while it is on, the
    /// running ones after it, with its own here ([`DAW_STARTUP`] order).
    pub startup: Part<4>,
    /// The TX lane nearest its late edge over the last 10 s
    /// ([`worst_lane`] over dstack's `tx_age_view`, dasio#237); `None`
    /// before a lane with one receiver was measured.
    pub tx_age: Option<TxAge>,
}

/// A Dante device name inside `Copy` facts: Dante names are at most 31
/// bytes; a longer one is cut at a char boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShortName {
    len: u8,
    bytes: [u8; 31],
}

impl ShortName {
    pub fn new(name: &str) -> Self {
        let mut end = name.len().min(31);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0u8; 31];
        bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        ShortName {
            len: u8::try_from(end).unwrap_or(31),
            bytes,
        }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

/// One TX lane's oldest packet over the last 10 s and the late edge its
/// receiver plays it at (dstack #500's `TxShared::tx_age_view`, dasio#237).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxAge {
    pub receiver: ShortName,
    pub age_ns: u64,
    pub edge_ns: u64,
}

/// Status calls in a row that may see dstack's TX age window unfed (its
/// `tx_age_looks` count standing still) before the window counts as stale:
/// the late watch feeds it once a second and the status hook runs once a
/// second, so one or two equal reads are timing, five are a dead watch.
pub const TX_AGE_STALE_READS: u32 = 5;

/// Whether dstack's TX age window is still fed, from its look count `looks`
/// and this reader's own memory `seen` (the count at its last call, and
/// how many calls in a row it stood still). A window no late watch feeds
/// keeps its last peaks: they must not stand as a current age.
pub fn tx_age_fed(seen: &mut (u64, u32), looks: u64) -> bool {
    if looks == seen.0 {
        seen.1 = seen.1.saturating_add(1);
    } else {
        *seen = (looks, 0);
    }
    looks > 0 && seen.1 < TX_AGE_STALE_READS
}

/// The worst lane of dstack's TX age view (dasio#237), each judged at its
/// own late edge (`late_threshold_ns`: the flow's latency, at least the
/// device's TX latency, plus a packet and the margin), the same edge the
/// lane's `over_latency` counts against. Never at the receiver's stated
/// latency: a receiver stating 0.25 ms plays a 1 ms sender's flow at 1 ms.
/// A lane with no edge (no flow, or its receiver's latency unknown) is not
/// judged.
pub fn worst_lane(view: &[dstack::media::tx::TxAgeLane]) -> Option<TxAge> {
    worst_tx_age(view.iter().filter_map(|l| {
        l.late_threshold_ns
            .map(|edge| (l.receiver.as_str(), edge, l.age_max_10s_ns))
    }))
}

/// The lane with the highest age/edge ratio, from `(receiver, late edge
/// ns, age over the last 10 s ns)` per lane: the one nearest to or furthest
/// past its edge, whatever that edge is (a 3 ms packet at a 1 ms edge
/// before a 10 ms one at a 7 ms edge). A lane counts with one receiver, a
/// known edge and a measured window only; `None` when none does. The window
/// belongs to the slot: a slot that got a new receiver within 10 s still
/// carries the old receiver's peaks.
pub fn worst_tx_age<'a>(
    lanes: impl IntoIterator<Item = (&'a str, u64, Option<u64>)>,
) -> Option<TxAge> {
    let mut worst: Option<TxAge> = None;
    for (receiver, edge_ns, age) in lanes {
        let Some(age_ns) = age else { continue };
        if receiver.is_empty() || edge_ns == 0 {
            continue;
        }
        let further = worst.is_none_or(|w| {
            u128::from(age_ns) * u128::from(w.edge_ns) > u128::from(w.age_ns) * u128::from(edge_ns)
        });
        if further {
            worst = Some(TxAge {
                receiver: ShortName::new(receiver),
                age_ns,
                edge_ns,
            });
        }
    }
    worst
}

/// The DAW kind's start-up counts ([`Facts::startup`]): the status key
/// (`startup_<key>`) and the tray line's label, in the split's order.
pub const DAW_STARTUP: [(&str, &str); 4] = [
    ("late_blocks", "late"),
    ("driver_missed", "driver missed"),
    ("stale", "stale"),
    ("waited", "waited"),
];

/// A bridge's start-up counts ([`BridgeFacts::startup`]), as
/// [`DAW_STARTUP`].
pub const BRIDGE_STARTUP: [(&str, &str); 3] = [
    ("to_asio_underruns", "underruns to ASIO"),
    ("from_asio_underruns", "underruns from ASIO"),
    ("resets", "resets"),
];

/// The tray line's start-up part (dasio#28): which counts it shows. In the
/// window they are the start-up's own; after it, the start-up window's
/// counts that are not 0, by `keys`' labels (nothing when all are 0).
fn startup_suffix<const N: usize>(p: &Part<N>, keys: &[(&str, &str); N]) -> String {
    if p.in_window {
        return format!(
            " · start-up (first {} min)",
            crate::startup::WINDOW.as_secs() / 60
        );
    }
    let counted: Vec<String> = keys
        .iter()
        .zip(p.startup)
        .filter(|&(_, n)| n > 0)
        .map(|((_, label), n)| format!("{label} {n}"))
        .collect();
    if counted.is_empty() {
        String::new()
    } else {
        format!(" · at start-up: {}", counted.join(", "))
    }
}

/// The status JSON's start-up keys (dasio#28), the object's last, each
/// after a comma: `phase`, and after the window the start-up window's
/// counts as `startup_<key>`.
fn startup_json<const N: usize>(p: &Part<N>, keys: &[(&str, &str); N]) -> String {
    if p.in_window {
        return ",\"phase\":\"start-up\"".to_string();
    }
    let counts: String = keys
        .iter()
        .zip(p.startup)
        .map(|((key, _), n)| format!(",\"startup_{key}\":{n}"))
        .collect();
    format!(",\"phase\":\"running\"{counts}")
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

/// The RX arrival mean under which the device's media clock likely runs
/// behind the network's (dasio#237): on 26.11.2026 FOH band read 0.18 ms
/// and mbc 0.19 ms, alex-nb 0.02 ms behind its USB NIC's lagging PTP
/// follower (0.00 ms before its silent RX flow, dasio#234).
pub const CLOCK_SIGN_MEAN_NS: u64 = 50_000;

/// What keeps an instance from low latency, in one look (dasio#237): the
/// PC profile, the TX path and the clock sign from the RX arrival mean.
/// Text and a JSON key only: the clock sign never sets the icon (the RX
/// arrival is a measurement, rx-arrival.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Every item holds.
    Ok,
    /// The items that do not, each with its number.
    Not(Vec<String>),
    /// Nothing failed, but no RX mean was measured yet.
    Checking,
}

impl Readiness {
    /// The tray line's part: only when an item fails, so a ready PC's line
    /// stays as it was.
    fn suffix(&self) -> String {
        match self {
            Readiness::Not(items) => format!(" · low latency NOT: {}", items.join(", ")),
            Readiness::Ok | Readiness::Checking => String::new(),
        }
    }

    /// The JSON key's value.
    pub fn key(&self) -> &'static str {
        match self {
            Readiness::Ok => "ok",
            Readiness::Not(_) => "not",
            Readiness::Checking => "checking",
        }
    }
}

/// The TX age the readiness line judges: none during the start-up window
/// (dasio#28; owner 21.11.2026: start misses do not count), since the
/// window's first look covers the device's start too.
fn shown_tx_age(f: &Facts) -> Option<TxAge> {
    if f.startup.in_window { None } else { f.tx_age }
}

/// The instance's [`Readiness`] from its PC profile problems (`None`: no
/// profile, not an item), its TX path, its worst TX lane's age against its
/// late edge over the last 10 s (`None`: not measured, not an
/// item; dasio#237) and its RX arrival.
pub fn readiness(
    pc_problems: Option<u32>,
    tx: TxPath,
    rx: Option<crate::rx_arrival::Report>,
    tx_age: Option<TxAge>,
) -> Readiness {
    let mut not = Vec::new();
    if let Some(n) = pc_problems.filter(|&n| n > 0) {
        not.push(format!("PC profile ({n})"));
    }
    match tx {
        TxPath::Kernel {
            no_slot: 0,
            failed: 0,
        } => {}
        TxPath::Kernel { no_slot, failed } => {
            not.push(format!("TX ({no_slot} to the socket, {failed} failed)"));
        }
        TxPath::Udp => not.push("TX (UDP)".to_string()),
    }
    // Late as dstack's `over_latency` judges it: at the edge itself the
    // packet still plays.
    if let Some(t) = tx_age.filter(|t| t.age_ns > t.edge_ns) {
        not.push(format!(
            "TX age {:.2} ms past the {:.2} ms {} plays it at",
            t.age_ns as f64 / 1e6,
            t.edge_ns as f64 / 1e6,
            t.receiver.as_str()
        ));
    }
    let mean = rx.and_then(|r| r.mean_ns);
    if let Some(m) = mean.filter(|&m| m < CLOCK_SIGN_MEAN_NS) {
        not.push(format!("clock (RX mean {:.2} ms)", m as f64 / 1e6));
    }
    if !not.is_empty() {
        Readiness::Not(not)
    } else if mean.is_none() {
        Readiness::Checking
    } else {
        Readiness::Ok
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
    let startup = startup_suffix(&f.startup, &DAW_STARTUP);
    let ready = readiness(f.pc_problems, f.tx, f.rx, shown_tx_age(f)).suffix();
    format!(
        "{} · {} @ {} kHz{} · in {:.2} / out {:.2} ms · late {}/min ({} total){startup}{ready}{rx}{tx}{pc}",
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
            "\"waited\":{},\"heartbeat_ms\":{},\"low_latency\":\"{}\",\"pc_profile_problems\":{},{}{}{}}}"
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
        readiness(f.pc_problems, f.tx, f.rx, shown_tx_age(f)).key(),
        f.pc_problems.map_or(-1, i64::from),
        crate::rx_arrival::json(f.rx),
        f.tx.json(),
        startup_json(&f.startup, &DAW_STARTUP),
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
    /// The current run's driver callbacks per 0.25 ms lateness bin, under
    /// two periods (dstack v0.6.173, dasio#120; `LATE_BIN_KEYS`).
    pub late_bins: [u64; LATE_BINS],
    /// Each direction's converter delay, ns (dstack v0.6.174, dasio#120;
    /// `Stats::converter_delay_ns`, the last player's): 0 until that
    /// direction's first player was made (the to-ASIO one at the driver's
    /// first open).
    pub to_asio_converter_delay_ns: u64,
    pub from_asio_converter_delay_ns: u64,
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
    /// The from-ASIO side's health since the device's start (dstack
    /// v0.6.167, dasio#120): the TX thread's fill of the from-ASIO
    /// channels (dstack's status `tx_pull`); `None` for a bridge with no
    /// channel from ASIO.
    pub from_asio_pull: Option<TxFillView>,
    /// The driver's reported input and output latency, frames (its last
    /// open): how long its card's side holds a sample each way (dasio#187).
    pub driver_input_latency_frames: u32,
    pub driver_output_latency_frames: u32,
    /// Each direction's target hold, ns (the players' `target_ms`): the
    /// fallback while no hold was measured (dasio#187); 0 when unknown.
    pub to_asio_target_ns: u64,
    pub from_asio_target_ns: u64,
    /// The start-up window (dasio#28): the underruns, `resets` and
    /// `late_bins` above are its counts while it is on, the running ones
    /// after it, with its own here ([`BRIDGE_STARTUP`] order).
    pub startup: Part<3>,
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
            late_bins: status.late_bins,
            to_asio_converter_delay_ns: 0,
            from_asio_converter_delay_ns: 0,
            pc_problems,
            tx,
            from_asio_age,
            rx: None,
            from_asio_pull: None,
            driver_input_latency_frames: status.input_latency_frames,
            driver_output_latency_frames: status.output_latency_frames,
            to_asio_target_ns: 0,
            from_asio_target_ns: 0,
            startup: Part::default(),
        }
    }
}

/// How long the bridge delays a sample each way, µs (dasio#187, owner
/// 18.11.2026: "od kedy sample pritecie do sg sietovky a kedy opusti dante
/// sietovku", and back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delays {
    /// From its arrival at the ASIO side's card until it leaves the Dante
    /// NIC: the driver's input latency + the from-ASIO hold (the measured
    /// block age, else the target) + the converter.
    pub asio_to_dante_us: u64,
    /// From its arrival at the Dante NIC until it leaves the ASIO side's
    /// card: the to-ASIO hold (the measured read wait, else the target) +
    /// the converter + the driver's output latency.
    pub dante_to_asio_us: u64,
}

/// The delays as dstack's figures carry them (dasio#187, dstack issue
/// 419): u32 us each, 0 while not known, a longer one held at u32::MAX
/// (never wrapped).
pub fn path_us(d: Option<&Delays>) -> (u32, u32) {
    let us = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
    d.map_or((0, 0), |d| (us(d.asio_to_dante_us), us(d.dante_to_asio_us)))
}

/// The driver runs and has told its rate and both latencies.
fn driver_told(f: &BridgeFacts) -> bool {
    f.running
        && f.rate != 0
        && f.driver_input_latency_frames != 0
        && f.driver_output_latency_frames != 0
}

/// The bridge's delays while its driver runs and has told its latencies;
/// `None` otherwise.
pub fn delays(f: &BridgeFacts) -> Option<Delays> {
    if !driver_told(f) {
        return None;
    }
    let frames = |n: u32| u64::from(n) * 1_000_000 / u64::from(f.rate);
    // A hold not measured counts its target; neither known: no figure
    // (a 0 would read as a fast bridge).
    let from_hold = match f.from_asio_age.and_then(|a| a.block_age_us) {
        Some(us) => us,
        None if f.from_asio_target_ns > 0 => f.from_asio_target_ns / 1_000,
        None => return None,
    };
    let to_hold = match f.rx.and_then(|r| r.wait).and_then(|w| w.mean_ns) {
        Some(ns) => ns / 1_000,
        None if f.to_asio_target_ns > 0 => f.to_asio_target_ns / 1_000,
        None => return None,
    };
    Some(Delays {
        asio_to_dante_us: frames(f.driver_input_latency_frames)
            .saturating_add(from_hold)
            .saturating_add(f.from_asio_converter_delay_ns / 1_000),
        dante_to_asio_us: to_hold
            .saturating_add(f.to_asio_converter_delay_ns / 1_000)
            .saturating_add(frames(f.driver_output_latency_frames)),
    })
}

/// The ASIO side's name in the line: the driver's, without " ASIO Driver"
/// or " ASIO" at its end ("Waves SoundGrid ASIO" reads "Waves SoundGrid").
pub fn side_name(driver: &str) -> &str {
    for suffix in [" ASIO Driver", " ASIO"] {
        if let Some(head) = driver.strip_suffix(suffix)
            && !head.is_empty()
        {
            return head;
        }
    }
    driver
}

/// The line's delays, one per direction, in ms with one decimal; empty
/// without [`delays`].
fn delays_suffix(f: &BridgeFacts) -> String {
    let Some(d) = delays(f) else {
        return String::new();
    };
    let side = side_name(&f.driver);
    format!(
        " · {side} → Dante {:.1} ms · Dante → {side} {:.1} ms",
        d.asio_to_dante_us as f64 / 1_000.0,
        d.dante_to_asio_us as f64 / 1_000.0
    )
}

/// The JSON keys of [`delays`], each followed by a comma; empty without.
fn delays_json(f: &BridgeFacts) -> String {
    match delays(f) {
        Some(d) => format!(
            "\"bridge_asio_to_dante_us\":{},\"bridge_dante_to_asio_us\":{},",
            d.asio_to_dante_us, d.dante_to_asio_us
        ),
        None => String::new(),
    }
}

/// The latencies the ASIO host is told, as the DAW object carries them
/// (dasio#248): a bridge's host is dasio itself and its driver tells it
/// `latency_in` / `latency_out` (frames at `sample_rate`, the driver's
/// rate). Only while the driver runs and has told both (never a 0 that
/// reads as no latency); each key followed by a comma, empty otherwise.
fn told_json(f: &BridgeFacts) -> String {
    if !driver_told(f) {
        return String::new();
    }
    format!(
        "\"latency_in\":{},\"latency_out\":{},\"sample_rate\":{},",
        f.driver_input_latency_frames, f.driver_output_latency_frames, f.rate
    )
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

/// The bridge status loop's underrun check (dstack v0.6.173, dasio#120),
/// once a counts tick, never from the pump or the driver's callback (it
/// formats and logs): the runner's `status` and both players' underruns
/// against the last tick's; when one rose, dstack WARNs which side, how
/// many, whose (the driver's callback lateness or the bridge's) and the
/// callbacks' lateness meanwhile, and that text is returned.
/// `from_asio_target_ms` is the from-ASIO player's target
/// (`Stats::target_ms`).
pub fn underrun_tick(
    watch: &mut UnderrunWatch,
    status: &dstack_asio::Status,
    to_asio_underruns: u64,
    from_asio_underruns: u64,
    from_asio_target_ms: f64,
) -> Option<String> {
    watch.look(
        UnderrunCounts::of(status, to_asio_underruns, from_asio_underruns),
        from_asio_target_ms,
    )
}

/// The bridge's lateness bin keys for its status JSON (dstack v0.6.173,
/// dasio#120), each followed by a comma: every bin, 0 included (a count).
fn late_bins_json(bins: &[u64; LATE_BINS]) -> String {
    LATE_BIN_KEYS
        .iter()
        .zip(bins)
        .map(|(key, n)| format!("\"{key}\":{n},"))
        .collect()
}

/// `ns` in whole us, rounded.
fn us_of_ns(ns: u64) -> u64 {
    (ns + 500) / 1000
}

/// The bridge's converter delay keys for its status JSON (dstack
/// v0.6.174, dasio#120), us, each followed by a comma: a direction's only
/// once its first player was made (never a 0 that reads as no delay).
fn converter_json(to_asio_ns: u64, from_asio_ns: u64) -> String {
    [
        ("to_asio_converter_delay_us", to_asio_ns),
        ("from_asio_converter_delay_us", from_asio_ns),
    ]
    .into_iter()
    .filter(|&(_, ns)| ns > 0)
    .map(|(key, ns)| format!("\"{key}\":{},", us_of_ns(ns)))
    .collect()
}

/// The bridge's from-ASIO pull key for its status JSON (dstack v0.6.167,
/// dasio#120), followed by a comma: the TX thread's fill counts since the
/// start, left out for a bridge with no channel from ASIO.
fn pull_json(pull: Option<TxFillView>) -> String {
    pull.map_or_else(String::new, |p| {
        format!(
            "\"from_asio_pull\":{{\"blocks\":{},\"rebootstraps\":{},\"skipped\":{},\"writes_after_send\":{},\"faults\":{}}},",
            p.blocks, p.rebootstraps, p.skipped, p.writes_after_send, p.faults
        )
    })
}

/// The tray line's from-ASIO pull part: nothing while the fill never
/// restarted, skipped, wrote after a send or faulted since the start.
fn pull_suffix(pull: Option<TxFillView>) -> String {
    match pull {
        Some(p)
            if p.rebootstraps > 0 || p.skipped > 0 || p.writes_after_send > 0 || p.faults > 0 =>
        {
            format!(
                " · from ASIO pull: restarts {}, skipped {}, late writes {}, faults {}",
                p.rebootstraps, p.skipped, p.writes_after_send, p.faults
            )
        }
        _ => String::new(),
    }
}

/// The from-ASIO pull's processor panicked: dstack never calls it again,
/// the from-ASIO channels stay silent until the device restarts.
fn pull_faulted(pull: Option<TxFillView>) -> bool {
    pull.is_some_and(|p| p.faults > 0)
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
        "bridge to \"{}\": {now}{}{} · underruns to ASIO {} / from ASIO {} · resets {}{}{}{}{}{}",
        f.driver,
        delays_suffix(f),
        readiness(f.pc_problems, f.tx, f.rx, None).suffix(),
        f.to_asio_underruns,
        f.from_asio_underruns,
        f.resets,
        startup_suffix(&f.startup, &BRIDGE_STARTUP),
        pull_suffix(f.from_asio_pull),
        crate::rx_arrival::suffix(f.rx),
        f.tx.suffix(),
        pc_suffix(f.pc_problems)
    )
}

/// The icon's state of a bridge instance: `ok` while the driver runs the
/// bridge (every frame through the filter, no PC profile problem, the
/// from-ASIO pull not faulted), `warn` otherwise.
pub fn bridge_state(f: &BridgeFacts) -> &'static str {
    if f.running
        && !f.tx.bad()
        && !f.pc_problems.is_some_and(|n| n > 0)
        && !pull_faulted(f.from_asio_pull)
    {
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
/// `state`, `kind` and every number (no DAW: `daw_attached` false), the
/// driver callbacks' lateness bins and each direction's converter delay
/// once its first player was made (dasio#120); the RX arrival and its
/// ASIO pull wait keys only once measured (dasio#128), the from-ASIO
/// pull's only with channels from ASIO (dasio#120).
pub fn bridge_json(f: &BridgeFacts) -> String {
    format!(
        concat!(
            "{{\"line\":\"{}\",\"state\":\"{}\",\"low_latency\":\"{}\",\"kind\":\"bridge\",\"daw_attached\":false,{}",
            "\"driver\":\"{}\",\"running\":{},\"reason\":\"{}\",\"rate\":{},",
            "\"buffer_frames\":{},\"to_asio_underruns\":{},\"from_asio_underruns\":{},",
            "\"resets\":{},{}{}{}\"pc_profile_problems\":{},{}{}{}{}{}{}}}"
        ),
        escaped(&bridge_line(f)),
        bridge_state(f),
        readiness(f.pc_problems, f.tx, f.rx, None).key(),
        told_json(f),
        escaped(&f.driver),
        f.running,
        f.reason_code.unwrap_or(""),
        f.rate,
        f.buffer_frames,
        f.to_asio_underruns,
        f.from_asio_underruns,
        f.resets,
        late_bins_json(&f.late_bins),
        delays_json(f),
        converter_json(f.to_asio_converter_delay_ns, f.from_asio_converter_delay_ns),
        f.pc_problems.map_or(-1, i64::from),
        age_json(f.from_asio_age),
        pull_json(f.from_asio_pull),
        crate::rx_arrival::json(f.rx),
        crate::rx_arrival::pull_wait_json(f.rx),
        f.tx.json(),
        startup_json(&f.startup, &BRIDGE_STARTUP),
    )
}

#[cfg(test)]
mod tests;
