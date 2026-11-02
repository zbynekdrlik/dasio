// SPDX-License-Identifier: GPL-3.0-or-later

//! The shared memory between the ASIO driver (in the DAW's process) and its
//! instance's device process (plan Task 5).
//!
//! One mapping per instance: a [`Header`], then two halves of buffers, each
//! half holding the RX buffers (the DAW's inputs) then the TX buffers (its
//! outputs), one per channel, `block` samples of `i32` (24 bits left-justified,
//! ASIO's `Int32LSB`), each 64-byte aligned. The device is the clock: per
//! block it fills the inputs of half `seq % 2`, bumps [`Header::seq`] and
//! signals the block event; the driver's thread hands that half to the DAW
//! and, once the DAW has written its outputs, stores the same number in
//! [`Header::ack`] and signals the ready event.

use std::sync::atomic::{AtomicU32, AtomicU64};

/// `"DASI"`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"DASI");
/// The layout's version.
pub const VERSION: u32 = 3;
/// [`Header::start`] of a block with no media time (no clock).
pub const NO_START: u64 = u64::MAX;
/// Every buffer starts on this boundary.
pub const ALIGN: usize = 64;
/// The most channels each way.
pub const MAX_CHANNELS: u32 = 64;
/// The blocks the device can run (its pump's).
pub const BLOCKS: [u32; 5] = [16, 32, 64, 128, 256];

/// The start of the mapping.
#[repr(C, align(64))]
#[derive(Debug, Default)]
pub struct Header {
    pub magic: AtomicU32,
    pub version: AtomicU32,
    /// The DAW's inputs (the device's RX channels).
    pub rx: AtomicU32,
    /// The DAW's outputs (the device's TX channels).
    pub tx: AtomicU32,
    /// Samples per buffer (the device's block, the ASIO buffer size).
    pub block: AtomicU32,
    /// Samples a second.
    pub rate: AtomicU32,
    /// The DAW's input latency the driver reports, samples: the age of a
    /// block's first input sample when the block reaches the DAW (B + P).
    pub latency_in: AtomicU32,
    /// The DAW's output latency, samples: from the block's hand-over until
    /// its first sample plays at a receiver (W + the TX latency we advertise).
    pub latency_out: AtomicU32,
    /// The device process's id (the driver checks it is alive).
    pub device_pid: AtomicU32,
    /// Blocks the device published (half `seq % 2` is the newest).
    pub seq: AtomicU64,
    /// The newest block whose outputs the DAW returned.
    pub ack: AtomicU64,
    /// The device's last sign of life, ms (the device's clock).
    pub heartbeat_ms: AtomicU64,
    /// 1 while a driver has the buffers.
    pub host_attached: AtomicU32,
    /// Blocks whose outputs came too late (sent as silence).
    pub late: AtomicU64,
    /// Blocks the driver never handed to the DAW (the device's seq moved on
    /// by more than one between two hand-overs).
    pub missed: AtomicU64,
    /// The media time of each half's block, its first input sample
    /// (written before its seq; [`NO_START`] without a clock): the DAW's
    /// sample position runs on it.
    pub start: [AtomicU64; 2],
}

/// The layout's shape: channels each way and the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub rx: u32,
    pub tx: u32,
    pub block: u32,
}

impl Shape {
    /// `None` past [`MAX_CHANNELS`] or with a block outside [`BLOCKS`].
    pub fn new(rx: u32, tx: u32, block: u32) -> Option<Shape> {
        (rx <= MAX_CHANNELS && tx <= MAX_CHANNELS && BLOCKS.contains(&block)).then_some(Shape {
            rx,
            tx,
            block,
        })
    }

    /// One buffer's bytes, rounded up to [`ALIGN`].
    fn buffer_bytes(self) -> usize {
        (self.block as usize * 4).next_multiple_of(ALIGN)
    }

    /// One half's bytes.
    fn half_bytes(self) -> usize {
        (self.rx + self.tx) as usize * self.buffer_bytes()
    }

    /// The whole mapping's bytes.
    pub fn size(self) -> usize {
        std::mem::size_of::<Header>().next_multiple_of(ALIGN) + 2 * self.half_bytes()
    }

    /// The byte offset of a buffer: `half` 0 or 1, the DAW's `input` or
    /// output, channel `ch` (from 0). `None` past the shape.
    pub fn buffer_offset(self, half: u32, input: bool, ch: u32) -> Option<usize> {
        let count = if input { self.rx } else { self.tx };
        if half > 1 || ch >= count {
            return None;
        }
        let index = if input { ch } else { self.rx + ch } as usize;
        Some(
            std::mem::size_of::<Header>().next_multiple_of(ALIGN)
                + half as usize * self.half_bytes()
                + index * self.buffer_bytes(),
        )
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Shared;

#[cfg(test)]
mod tests;
