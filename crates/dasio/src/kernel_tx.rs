//! Audio TX through dasio's own NDIS filter (dasio#1, the owner's
//! decision of 2.11.2026): dstack hands each TX tick's finished Ethernet
//! frames to the filter's `IOCTL_DASIO_TX_SEND`, which sends them straight
//! to the miniport, below tcpip and WFP. On the FOH PC with Realtek 10.79,
//! Winsock refused socket sends with WSAENOBUFS about 32 times a day and
//! held on-time sends for up to 6 ms; the filter's own frames did not
//! stall (kernel test stream, 2.11.2026).
//!
//! This part is portable and tested everywhere: the IOCTL's code and the
//! packing of a batch into its input. The device itself is
//! [`windows`](self::windows) only.

#[cfg(windows)]
pub mod windows;

/// The filter's control device (`LINKNAME_STRING` in filter.h).
pub const DEVICE_PATH: &str = r"\\.\dasiopacer";

/// The smallest and largest frame the filter takes (`pacer_tx_next`):
/// Ethernet + IPv4 + UDP headers, and a full 1500-byte MTU frame.
pub const MIN_FRAME: usize = 42;
pub const MAX_FRAME: usize = 1514;

/// The input's header: the adapter's IfIndex and a reserved word.
pub const HEADER: usize = 8;

/// Windows' `CTL_CODE(device, function, method, access)`: its fields
/// never overlap, so the sum is the macro's bitwise or.
pub const fn ctl_code(device: u32, function: u32, method: u32, access: u32) -> u32 {
    device * 0x1_0000 + access * 0x4000 + function * 4 + method
}

/// `IOCTL_DASIO_TX_SEND` (filteruser.h): FILE_DEVICE_PHYSICAL_NETCARD,
/// function 21, METHOD_BUFFERED, FILE_WRITE_ACCESS.
pub const IOCTL_DASIO_TX_SEND: u32 = ctl_code(0x17, 21, 0, 2);

/// `IOCTL_DASIO_KTX_STATUS` (filteruser.h): `_NDIS_CONTROL_CODE(18,
/// METHOD_BUFFERED)`, FILE_ANY_ACCESS. One `DASIO_KTX_STATUS` per bound
/// adapter.
pub const IOCTL_DASIO_KTX_STATUS: u32 = ctl_code(0x17, 18, 0, 0);

/// The u64 fields of a `DASIO_KTX_STATUS` record, by its Version.
fn status_fields(version: u64) -> Option<usize> {
    match version {
        1 => Some(16),
        2 => Some(20),
        3 => Some(25),
        4 => Some(27),
        _ => None,
    }
}

/// What dasio reads of one adapter's `DASIO_KTX_STATUS`. Fields a
/// record's version does not carry read 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KtxStatus {
    pub version: u64,
    pub if_index: u32,
    /// Frames of the slots completed with an error (the miniport refused
    /// them: lost at the receivers).
    pub failed: u64,
    /// Frames not completed yet.
    pub out: u64,
    pub tx_frames: u64,
    /// Media frames the filter did not take (every slot out, or the bound
    /// on frames out): they went to the UDP socket.
    pub tx_no_slot: u64,
    pub tx_max_out: u64,
    pub max_out: u64,
    pub fail_events: u64,
    pub last_fail_status: u64,
    /// The filter's interrupt time at the reading (100 ns).
    pub now: u64,
    /// Interrupt time of the last failure (0: none yet).
    pub last_fail_time: u64,
    /// Version 4: socket fallbacks more than 1 s apart, and the last one's
    /// interrupt time (0: none yet).
    pub no_slot_events: u64,
    pub last_no_slot_time: u64,
}

/// Bound adapters dasio reads at most (the reply buffer's size).
pub const MAX_STATUS_RECORDS: usize = 16;

/// The records of an `IOCTL_DASIO_KTX_STATUS` reply, in order. A record
/// of an unknown version, or one cut short, ends the walk. At most
/// [`MAX_STATUS_RECORDS`] are read.
pub fn parse_status(reply: &[u8]) -> Vec<KtxStatus> {
    let word = |at: usize| u64::from_le_bytes(reply[at..at + 8].try_into().unwrap());
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= reply.len() && out.len() < MAX_STATUS_RECORDS {
        let version = word(at);
        let Some(fields) = status_fields(version) else {
            break;
        };
        if at + fields * 8 > reply.len() {
            break;
        }
        let f = |i: usize| if i < fields { word(at + i * 8) } else { 0 };
        out.push(KtxStatus {
            version,
            if_index: f(1) as u32,
            failed: f(10),
            out: f(11),
            tx_frames: f(17),
            tx_no_slot: f(18),
            tx_max_out: f(20),
            max_out: f(21),
            fail_events: f(22),
            last_fail_status: f(23),
            now: f(6),
            last_fail_time: f(24),
            no_slot_events: f(25),
            last_no_slot_time: f(26),
        });
        at += fields * 8;
    }
    out
}

/// One window between two readings of the instance's adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KtxReport {
    pub frames: u64,
    pub no_slot: u64,
    pub failed: u64,
    pub fail_events: u64,
    pub max_out: u64,
    pub tx_max_out: u64,
    pub out: u64,
    pub last_fail_status: u64,
    /// Socket-fallback bursts in the window (version 4; 0 before).
    pub no_slot_events: u64,
    /// How long before the reading the last failure / fallback was, in ms
    /// (None: none yet, or the filter does not say).
    pub last_fail_ago_ms: Option<u64>,
    pub last_no_slot_ago_ms: Option<u64>,
}

/// Milliseconds from an interrupt time `at` to `now` (both 100 ns);
/// None when `at` is 0 (never) or later than `now`.
pub fn ago_ms(now: u64, at: u64) -> Option<u64> {
    if at == 0 || at > now {
        return None;
    }
    Some((now - at) / 10_000)
}

impl KtxReport {
    /// Frames went to the socket or were lost in this window.
    pub fn bad(&self) -> bool {
        self.no_slot > 0 || self.failed > 0
    }
}

/// Turns successive readings into per-window reports for one adapter.
#[derive(Debug, Default)]
pub struct StatusWatch {
    last: Option<KtxStatus>,
}

impl StatusWatch {
    /// Takes a reading; the report covers the time since the previous one.
    /// The first reading, a missing adapter, or counters that went back
    /// (the filter restarted) give none and start a new baseline.
    pub fn step(&mut self, reply: &[KtxStatus], if_index: u32) -> Option<KtxReport> {
        let Some(now) = reply.iter().find(|s| s.if_index == if_index).copied() else {
            self.last = None;
            return None;
        };
        let prev = self.last.replace(now)?;
        if now.tx_frames < prev.tx_frames
            || now.tx_no_slot < prev.tx_no_slot
            || now.failed < prev.failed
            || now.fail_events < prev.fail_events
            || now.no_slot_events < prev.no_slot_events
        {
            return None;
        }
        Some(KtxReport {
            frames: now.tx_frames - prev.tx_frames,
            no_slot: now.tx_no_slot - prev.tx_no_slot,
            failed: now.failed - prev.failed,
            fail_events: now.fail_events - prev.fail_events,
            max_out: now.max_out,
            tx_max_out: now.tx_max_out,
            out: now.out,
            last_fail_status: now.last_fail_status,
            no_slot_events: now.no_slot_events - prev.no_slot_events,
            last_fail_ago_ms: ago_ms(now.now, now.last_fail_time),
            last_no_slot_ago_ms: ago_ms(now.now, now.last_no_slot_time),
        })
    }
}

/// Why a batch could not be packed (it then goes through the sockets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// A frame outside [`MIN_FRAME`]..=[`MAX_FRAME`], or not IPv4 over
    /// Ethernet II (the filter would refuse the whole input).
    Frame(usize),
    /// The batch does not fit the buffer's capacity (packing never
    /// allocates: it runs on the TX thread).
    Full,
}

/// Packs `frames` for `if_index` into `buf`, replacing what it held:
/// the header, then each frame as a 2-byte little-endian length and its
/// bytes. Never grows `buf`.
pub fn pack(frames: &[&[u8]], if_index: u32, buf: &mut Vec<u8>) -> Result<(), PackError> {
    let need = HEADER + frames.iter().map(|f| 2 + f.len()).sum::<usize>();
    for (i, f) in frames.iter().enumerate() {
        let ipv4 = f.len() >= MIN_FRAME
            && f[12] == 0x08
            && f[13] == 0x00
            && f[14] >> 4 == 4
            && f[14] & 0x0F >= 5;
        if !ipv4 || f.len() > MAX_FRAME {
            return Err(PackError::Frame(i));
        }
    }
    if need > buf.capacity() {
        return Err(PackError::Full);
    }
    buf.clear();
    buf.extend_from_slice(&if_index.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    for f in frames {
        buf.extend_from_slice(&(f.len() as u16).to_le_bytes());
        buf.extend_from_slice(f);
    }
    Ok(())
}

/// Marks the first `accepted` frames sent (the filter takes them in
/// order); the rest stay unsent for the socket path.
pub fn mark(accepted: u32, sent: &mut [bool]) {
    let n = (accepted as usize).min(sent.len());
    sent[..n].fill(true);
    sent[n..].fill(false);
}

/// When a sender whose adapter vanished (the filter answers "not found",
/// e.g. after the NIC was re-enumerated with a new IfIndex) looks it up
/// again: at the first miss, then at most once a second, so the TX thread
/// never runs `GetAdaptersAddresses` on every tick.
#[derive(Debug, Default)]
pub struct Relookup {
    last_ms: Option<u64>,
}

impl Relookup {
    /// Whether to look the adapter up at `now_ms` (any monotonic ms clock);
    /// counts as done when it says yes.
    pub fn due(&mut self, now_ms: u64) -> bool {
        if self
            .last_ms
            .is_some_and(|l| now_ms.saturating_sub(l) < 1000)
        {
            return false;
        }
        self.last_ms = Some(now_ms);
        true
    }
}

#[cfg(test)]
mod tests;
