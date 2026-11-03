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

/// Windows' `CTL_CODE(device, function, method, access)`.
pub const fn ctl_code(device: u32, function: u32, method: u32, access: u32) -> u32 {
    (device << 16) | (access << 14) | (function << 2) | method
}

/// `IOCTL_DASIO_TX_SEND` (filteruser.h): FILE_DEVICE_PHYSICAL_NETCARD,
/// function 21, METHOD_BUFFERED, FILE_WRITE_ACCESS.
pub const IOCTL_DASIO_TX_SEND: u32 = ctl_code(0x17, 21, 0, 2);

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

#[cfg(test)]
mod tests;
