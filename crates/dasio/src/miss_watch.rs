//! A watch on outputs of another ASIO host that this device receives over
//! Dante (dasio#207). Master Live plays through the Yamaha card's own ASIO
//! driver. When it misses a buffer, the card still sends every Dante packet
//! on time, with a broken block inside: dcontroller's timing shows nothing
//! and no software on that path writes a log. The miss stays only in the
//! audio, as one of two shapes:
//!
//! - **zero**: digital silence of a whole number of the host's blocks
//!   ([`TOLERANCE`] samples either way, at most [`MAX_BLOCKS`]) where the
//!   channel carried signal (its recent peak above [`FLOOR`]);
//! - **repeat**: the samples of the block before, exactly, for at least a
//!   block less [`TOLERANCE`] and at most [`MAX_BLOCKS`] blocks, varying
//!   within the run (not zero, not one held value), on a channel whose
//!   recent peak is above [`FLOOR`]. The upper bound keeps a signal that is
//!   itself periodic in the block (a bit-exact test tone) out.
//!
//! Live audio never does either, and an RF drop on a microphone is never
//! sample-exact. A Dante packet hole is not this either: dstack logs RX holes
//! itself, and a hole's zero run is a packet's length.
//!
//! Those two shapes are all it sees of live audio. A continuous test tone on
//! channels of their own (`tone`, owner 21.11.2026) is checked sample by
//! sample instead ([`tone`]): any sample off the sine (a click, a short gap
//! of any length, a slipped sample, a repeated block) or a step in its level
//! is a break.
//!
//! Real program audio has no test tone, and a master's miss on the Yamaha
//! card is neither of the first two shapes: a block's worth of signal is
//! skipped or repeated, a step between two valid waveforms. Channels in
//! `program` are checked for that step ([`program`]), by the fourth
//! difference of the samples against the quiet before and after it, and
//! for a loop ([`looped`]): the same samples again and again, a device
//! replaying its buffer while it is not served.
//!
//! The pump thread runs [`Watch::check_block`] (through [`Watched`], a
//! [`Process`] in front of the device's own processor): per-channel state is
//! allocated at the start, the breaks go into [`Events`] (the tone's into
//! [`tone::ToneEvents`]), fixed single-writer rings of atomics ([`Ring`]).
//! The run loop drains them ([`Ring::drain`]), groups the zero and repeat
//! breaks of one block over channels ([`group`]) and logs them. The config
//! is the instance folder's `watch.toml` ([`config`]), not a `device.toml`
//! table: dstack's dtray refuses keys it does not know there.

pub mod judge;
pub mod looped;
pub mod program;
pub mod silence;
pub mod stall;
pub mod tone;

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dstack_audio_io::{Block, Process};

pub use looped::Loop;
pub use program::{ProgramJump, Programs};
pub use tone::{ToneBreak, ToneKind, Tones};

/// The instance folder's file that turns the watch on.
pub const FILE: &str = "watch.toml";
/// Samples a break's length may differ from a whole number of blocks.
pub const TOLERANCE: usize = 4;
/// The most blocks a zero run may span and still be read as host misses.
pub const MAX_BLOCKS: usize = 8;
/// A channel's recent peak must be above this (linear; -60 dBFS) for a
/// break to count: a quiet or muted channel says nothing.
pub const FLOOR: f64 = 1e-3;
/// The recent peak's time constant, s.
pub const PEAK_HOLD_S: f64 = 0.05;
/// Breaks kept between two drains; more are counted as lost.
pub const CAPACITY: usize = 256;
/// The host block sizes the config takes, samples.
pub const MIN_BLOCK: usize = 16;
pub const MAX_BLOCK: usize = 8192;
/// The watched channels at most.
pub const MAX_CHANNELS: usize = 16;

/// A break's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zero,
    Repeat,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Zero => "zero",
            Kind::Repeat => "repeat",
        }
    }
}

/// One break on one watched channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Break {
    /// The channel's place in the config's `rx` list.
    pub channel: usize,
    pub kind: Kind,
    /// The media sample position of the break's first sample.
    pub at: u64,
    /// Its length, samples.
    pub len: u32,
}

/// The `watch.toml` as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Whose outputs these are (for the log, e.g. "master").
    pub name: String,
    /// The host's ASIO block, samples at the device rate.
    pub block: usize,
    /// The watched channels: (the device's RX index, its name).
    pub rx: Names,
    /// The channels that carry a continuous test tone ([`tone`]), the same
    /// way; a channel may be in both lists.
    pub tone: Names,
    /// The channels of program audio ([`program`]): checked for the step a
    /// missed host buffer leaves; a channel may be in the other lists too.
    pub program: Names,
}

/// Channels as a config names them: (the device's RX index, its name).
pub type Names = Vec<(usize, String)>;

/// The keys of `watch.toml`.
const KEYS: [&str; 5] = ["name", "block", "rx", "tone", "program"];

/// The list `key` of `table` against the device's RX channel names: `None`
/// without the key, else `least` to [`MAX_CHANNELS`] names, each an RX
/// channel of the device, each once.
fn names(
    table: &toml::Table,
    key: &str,
    least: usize,
    device_rx: &[String],
) -> Result<Option<Names>, String> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let toml::Value::Array(items) = value else {
        return Err(format!(
            "{FILE}: `{key}` must be a list of RX channel names"
        ));
    };
    if items.len() < least || items.len() > MAX_CHANNELS {
        return Err(format!(
            "{FILE}: `{key}` must name {least} to {MAX_CHANNELS} channels"
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let toml::Value::String(want) = item else {
            return Err(format!("{FILE}: `{key}` entries must be strings"));
        };
        let Some(i) = device_rx.iter().position(|n| n == want) else {
            return Err(format!(
                "{FILE}: `{key}` names \"{want}\", which is no RX channel of this device"
            ));
        };
        if out.iter().any(|(j, _)| *j == i) {
            return Err(format!("{FILE}: `{key}` names \"{want}\" twice"));
        }
        out.push((i, want.clone()));
    }
    Ok(Some(out))
}

/// Reads `watch.toml`'s `text` against the device's RX channel names.
/// `rx` names 1 to [`MAX_CHANNELS`] channels; with a `tone` list (1 to
/// [`MAX_CHANNELS`]) or a `program` list (the same) it may be empty or left
/// out (a tone-only or program-only watch).
pub fn config(text: &str, device_rx: &[String]) -> Result<Config, String> {
    let table: toml::Table = text.parse().map_err(|e| format!("{FILE}: {e}"))?;
    if let Some(k) = table.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!(
            "{FILE}: unknown key `{k}` (known: {})",
            KEYS.join(", ")
        ));
    }
    let name = match table.get("name") {
        Some(toml::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Err(format!("{FILE}: `name` must be a non-empty string")),
    };
    let block = match table.get("block") {
        Some(toml::Value::Integer(n)) => usize::try_from(*n)
            .ok()
            .filter(|b| (MIN_BLOCK..=MAX_BLOCK).contains(b))
            .ok_or_else(|| format!("{FILE}: `block` {n} is not {MIN_BLOCK} to {MAX_BLOCK}"))?,
        _ => return Err(format!("{FILE}: `block` must be an integer (samples)")),
    };
    let tone = names(&table, "tone", 1, device_rx)?;
    let program = names(&table, "program", 1, device_rx)?;
    // A tone-only or program-only watch needs no `rx`.
    let other = tone.is_some() || program.is_some();
    let rx = match (names(&table, "rx", usize::from(!other), device_rx)?, other) {
        (Some(rx), _) => rx,
        (None, true) => Vec::new(),
        (None, false) => {
            return Err(format!("{FILE}: `rx` must be a list of RX channel names"));
        }
    };
    Ok(Config {
        name,
        block,
        rx,
        tone: tone.unwrap_or_default(),
        program: program.unwrap_or_default(),
    })
}

/// A watch as the run holds it: its config and the rings (and the tone
/// figures) its pump side writes: one set for the run; each serving makes
/// its own [`Watch`].
pub struct Setup {
    pub cfg: Config,
    pub events: Arc<Events>,
    pub tones: Arc<Tones>,
    pub programs: Arc<Programs>,
}

impl Setup {
    pub fn new(cfg: Config) -> Self {
        let tones = Arc::new(Tones::new(cfg.tone.len()));
        let programs = Arc::new(Programs::new(cfg.program.len()));
        Self {
            cfg,
            events: Arc::new(Events::default()),
            tones,
            programs,
        }
    }

    /// The pump side for a serving at the device's `rate`.
    pub fn watch(&self, rate: u32) -> Watch {
        Watch::new(&self.cfg, rate, Arc::clone(&self.events))
            .with_outputs(Arc::clone(&self.tones), Arc::clone(&self.programs))
    }
}

/// The watch's config in the instance folder `dir`: `None` without the
/// file, an error naming the file or the key when it does not read.
pub fn setup(dir: &std::path::Path, device_rx: &[String]) -> Result<Option<Config>, String> {
    let path = dir.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => config(&text, device_rx).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The device's RX channel names as it lists them now (dasio#207): the
/// state file's, where a Dante rename (dcontroller) is kept, over the
/// config's defaults. The watch's names are matched against these.
pub fn rx_names_now(cfg: &dstack::device::DeviceConfig) -> Result<Vec<String>, String> {
    dstack::device::DeviceState::load_or_default(cfg)
        .map(|state| state.rx)
        .map_err(|e| e.to_string())
}

/// The names of `g`'s channels in the config's words, in config order.
pub fn channel_names(g: &Group, cfg: &Config) -> String {
    g.channels
        .iter()
        .filter_map(|&c| cfg.rx.get(c).map(|(_, n)| n.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The names of the `program` channels `channels` (places in the config's
/// `program`) in the config's words, in the order given.
pub fn program_names(channels: &[usize], cfg: &Config) -> String {
    channels
        .iter()
        .filter_map(|&c| cfg.program.get(c).map(|(_, n)| n.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a recent peak is signal enough for a break to count.
pub fn loud(peak: f64) -> bool {
    peak > FLOOR
}

/// Whether an exact repeat of `run` samples has a host miss's length: a
/// block less [`TOLERANCE`] up to [`MAX_BLOCKS`] blocks.
pub fn repeat_length(run: usize, block: usize) -> bool {
    run + TOLERANCE >= block && run <= MAX_BLOCKS * block + TOLERANCE
}

/// Of the slots read from `start` to `h`, how many the writer may have
/// overwritten during the read when its head is `h2` by then: those older
/// than `h2` less the ring's size.
pub fn overwritten(start: u64, h: u64, h2: u64) -> u64 {
    h2.saturating_sub(CAPACITY as u64)
        .saturating_sub(start)
        .min(h - start)
}

/// Whether a zero run of `len` samples is a whole number of `block`s.
pub fn whole_blocks(len: usize, block: usize) -> bool {
    let k = (len + block / 2) / block;
    (1..=MAX_BLOCKS).contains(&k) && len.abs_diff(k * block) <= TOLERANCE
}

/// What a [`Ring`] slot holds: an item packed into `W` words.
pub trait Slot<const W: usize> {
    fn pack(&self) -> [u64; W];
    fn unpack(words: [u64; W]) -> Self;
}

/// Items between the pump thread and the run loop: one writer (the pump),
/// one reader (the loop), each item packed into a slot of `W` atomics.
pub struct Ring<T, const W: usize> {
    slots: Box<[[AtomicU64; W]]>,
    head: AtomicU64,
    item: PhantomData<fn() -> T>,
}

/// The zero and repeat breaks. A slot is `[at, info]`; `info` packs the
/// channel (bits 40-55), the kind (bit 32) and the length (bits 0-31).
pub type Events = Ring<Break, 2>;

impl<T, const W: usize> Default for Ring<T, W> {
    fn default() -> Self {
        Self {
            slots: (0..CAPACITY)
                .map(|_| std::array::from_fn::<AtomicU64, W, _>(|_| AtomicU64::new(0)))
                .collect(),
            head: AtomicU64::new(0),
            item: PhantomData,
        }
    }
}

impl Slot<2> for Break {
    fn pack(&self) -> [u64; 2] {
        let kind = match self.kind {
            Kind::Zero => 0,
            Kind::Repeat => 1,
        };
        // The three fields never overlap: a sum, so no `|`/`^` twin.
        [
            self.at,
            ((self.channel as u64 & 0xFFFF) << 40) + (kind << 32) + u64::from(self.len),
        ]
    }

    fn unpack([at, info]: [u64; 2]) -> Self {
        Break {
            channel: ((info >> 40) & 0xFFFF) as usize,
            kind: if (info >> 32) & 1 == 1 {
                Kind::Repeat
            } else {
                Kind::Zero
            },
            at,
            len: (info & 0xFFFF_FFFF) as u32,
        }
    }
}

impl<T: Slot<W>, const W: usize> Ring<T, W> {
    /// The writer's side (the pump thread only): never blocks, never
    /// allocates; a full ring overwrites its oldest item.
    pub fn push(&self, item: T) {
        let h = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[(h % CAPACITY as u64) as usize];
        for (cell, word) in slot.iter().zip(item.pack()) {
            cell.store(word, Ordering::Relaxed);
        }
        self.head.store(h + 1, Ordering::Release);
    }

    /// The reader's side: the items pushed since `tail` (the reader's own
    /// count), oldest first, into `out`; returns how many were lost to the
    /// ring's size. A slot the writer reached again while it was read is
    /// dropped and counted as lost. `tail` moves to the head.
    pub fn drain(&self, tail: &mut u64, out: &mut Vec<T>) -> u64 {
        self.drain_with(tail, out, || {})
    }

    /// [`Ring::drain`] with `during` run between the read and the check of
    /// the head: a test's stand-in for the writer pushing meanwhile.
    fn drain_with(&self, tail: &mut u64, out: &mut Vec<T>, during: impl FnOnce()) -> u64 {
        let h = self.head.load(Ordering::Acquire);
        let mut lost = (h - *tail).saturating_sub(CAPACITY as u64);
        let start = *tail + lost;
        let first = out.len();
        for i in start..h {
            let slot = &self.slots[(i % CAPACITY as u64) as usize];
            out.push(T::unpack(std::array::from_fn(|k| {
                slot[k].load(Ordering::Relaxed)
            })));
        }
        during();
        let h2 = self.head.load(Ordering::Acquire);
        let bad = overwritten(start, h, h2);
        out.drain(first..first + bad as usize);
        lost += bad;
        *tail = h;
        lost
    }
}

/// One watched channel's state.
struct Channel {
    rx: usize,
    /// The last `block` samples, for the repeat check.
    ring: Box<[f64]>,
    pos: usize,
    /// The sample before (a repeat must vary within its run).
    last: f64,
    zrun: usize,
    zstart: u64,
    zpeak: f64,
    rrun: usize,
    rstart: u64,
    rvaries: bool,
    peak: f64,
}

impl Channel {
    fn new(rx: usize, block: usize) -> Self {
        Self {
            rx,
            ring: vec![0.0; block].into_boxed_slice(),
            pos: 0,
            last: 0.0,
            zrun: 0,
            zstart: 0,
            zpeak: 0.0,
            rrun: 0,
            rstart: 0,
            rvaries: false,
            peak: 0.0,
        }
    }

    /// Back to the start state, allocating nothing (the pump thread).
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.last = 0.0;
        self.zrun = 0;
        self.zstart = 0;
        self.zpeak = 0.0;
        self.rrun = 0;
        self.rstart = 0;
        self.rvaries = false;
        self.peak = 0.0;
    }

    fn feed(&mut self, x: f64, at: u64, decay: f64, out: &Events, ch: usize) {
        let block = self.ring.len();
        if x == 0.0 {
            if self.zrun == 0 {
                self.zstart = at;
                self.zpeak = self.peak;
            }
            self.zrun += 1;
        } else if self.zrun != 0 {
            if loud(self.zpeak) && whole_blocks(self.zrun, block) {
                out.push(Break {
                    channel: ch,
                    kind: Kind::Zero,
                    at: self.zstart,
                    len: self.zrun as u32,
                });
            }
            self.zrun = 0;
        }
        let before = self.ring[self.pos];
        if x != 0.0 && x == before {
            // Variation counts only between samples of the run: the sample
            // before it may differ (a step from one constant to another).
            if self.rrun == 0 {
                self.rstart = at;
                self.rvaries = false;
            } else if x != self.last {
                self.rvaries = true;
            }
            self.rrun += 1;
        } else if self.rrun != 0 {
            if self.rvaries && repeat_length(self.rrun, block) && loud(self.peak) {
                out.push(Break {
                    channel: ch,
                    kind: Kind::Repeat,
                    at: self.rstart,
                    len: self.rrun as u32,
                });
            }
            self.rrun = 0;
        }
        self.ring[self.pos] = x;
        self.pos = if self.pos + 1 == block {
            0
        } else {
            self.pos + 1
        };
        self.last = x;
        self.peak = x.abs().max(self.peak * decay);
    }
}

/// The pump side of the watch.
pub struct Watch {
    channels: Vec<Channel>,
    events: Arc<Events>,
    decay: f64,
    /// The next block's position when the block has no media time.
    next: u64,
    /// The tone channels, their breaks and figures, and the rate's limits.
    tones: Vec<tone::Channel>,
    tone_out: Arc<Tones>,
    limits: tone::Limits,
    /// The program channels, their events and figures.
    programs: Vec<program::Channel>,
    /// The same channels' loop check, one per program channel.
    loops: Vec<looped::Channel>,
    program_out: Arc<Programs>,
}

impl Watch {
    /// A watch of `cfg`'s channels at `rate` (Hz), breaks into `events`;
    /// the tone breaks go into a ring of its own ([`Setup::watch`] gives it
    /// the setup's).
    pub fn new(cfg: &Config, rate: u32, events: Arc<Events>) -> Self {
        Self {
            channels: cfg
                .rx
                .iter()
                .map(|(i, _)| Channel::new(*i, cfg.block))
                .collect(),
            events,
            decay: (-1.0 / (PEAK_HOLD_S * f64::from(rate.max(1)))).exp(),
            next: 0,
            tones: cfg
                .tone
                .iter()
                .map(|(i, _)| tone::Channel::new(*i))
                .collect(),
            tone_out: Arc::new(Tones::new(cfg.tone.len())),
            limits: tone::Limits::new(rate),
            programs: cfg
                .program
                .iter()
                .map(|(i, _)| program::Channel::new(*i))
                .collect(),
            loops: cfg
                .program
                .iter()
                .map(|_| looped::Channel::new(cfg.block))
                .collect(),
            program_out: Arc::new(Programs::new(cfg.program.len())),
        }
    }

    /// The watch with its tone and program events and figures going into
    /// `tones` and `programs` (its [`Setup`]'s, one per channel of the
    /// config's lists). Private: only [`Setup::watch`] pairs them, from one
    /// config, so the pump never indexes past the figures.
    fn with_outputs(mut self, tones: Arc<Tones>, programs: Arc<Programs>) -> Self {
        self.tone_out = tones;
        self.program_out = programs;
        self
    }

    /// Checks `block`'s watched inputs. A jump in the media position (blocks
    /// the engine skipped) starts every channel again.
    #[cfg_attr(dasio_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn check_block(&mut self, block: &Block<'_>) {
        let start = block.start().unwrap_or(self.next);
        if start != self.next {
            self.forget();
        }
        for (ch, c) in self.channels.iter_mut().enumerate() {
            for (i, &x) in block.input(c.rx).iter().enumerate() {
                c.feed(x, start + i as u64, self.decay, &self.events, ch);
            }
        }
        for (ch, t) in self.tones.iter_mut().enumerate() {
            for (i, &x) in block.input(t.rx).iter().enumerate() {
                t.feed(x, start + i as u64, &self.limits, &self.tone_out.events, ch);
            }
            t.publish(self.tone_out.stats(ch));
        }
        let both = self.programs.iter_mut().zip(self.loops.iter_mut());
        for (ch, (p, l)) in both.enumerate() {
            for (i, &x) in block.input(p.rx).iter().enumerate() {
                p.feed(x, start + i as u64, &self.program_out.events, ch);
                l.feed(x, start + i as u64, &self.program_out.loops, ch);
            }
            p.publish(self.program_out.stats(ch));
        }
        self.next = start + block.frames() as u64;
    }

    /// Starts every channel again (a discontinuity: a reopen, a jump).
    pub fn forget(&mut self) {
        for c in &mut self.channels {
            c.reset();
        }
        for t in &mut self.tones {
            t.reset();
        }
        for p in &mut self.programs {
            p.reset();
        }
        for l in &mut self.loops {
            l.reset();
        }
    }
}

/// The device's processor with the watch in front of it.
pub struct Watched<P> {
    inner: P,
    watch: Option<Watch>,
}

impl<P> Watched<P> {
    pub fn new(inner: P, watch: Option<Watch>) -> Self {
        Self { inner, watch }
    }
}

impl<P: Process> Process for Watched<P> {
    #[cfg_attr(dasio_rtsan, sanitize(realtime = "nonblocking"))]
    fn process(&mut self, block: &mut Block<'_>) {
        if let Some(w) = self.watch.as_mut() {
            w.check_block(block);
        }
        self.inner.process(block);
    }

    fn discontinuity(&mut self) {
        if let Some(w) = self.watch.as_mut() {
            w.forget();
        }
        self.inner.discontinuity();
    }
}

/// The breaks of one moment over the watched channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub kind: Kind,
    /// The earliest break's position and the longest break's length.
    pub at: u64,
    pub len: u32,
    /// The channels (places in the config's `rx`), ascending.
    pub channels: Vec<usize>,
}

/// `breaks` (in push order) grouped: breaks of one kind whose starts lie
/// within `block` samples of the group's first are one group.
pub fn group(breaks: &[Break], block: usize) -> Vec<Group> {
    let mut sorted = breaks.to_vec();
    sorted.sort_by_key(|b| (b.at, b.channel));
    let mut out: Vec<Group> = Vec::new();
    for b in sorted {
        if let Some(g) = out
            .iter_mut()
            .rev()
            .find(|g| g.kind == b.kind && b.at - g.at <= block as u64)
        {
            if !g.channels.contains(&b.channel) {
                g.channels.push(b.channel);
                g.channels.sort_unstable();
            }
            g.len = g.len.max(b.len);
            continue;
        }
        out.push(Group {
            kind: b.kind,
            at: b.at,
            len: b.len,
            channels: vec![b.channel],
        });
    }
    out
}

/// How long ago position `at` was, ms, when the media position is `now`
/// (both samples at `rate`); 0 for a position not in the past.
pub fn age_ms(at: u64, now: u64, rate: u32) -> u64 {
    now.saturating_sub(at) * 1000 / u64::from(rate.max(1))
}

#[cfg(test)]
mod tests;
