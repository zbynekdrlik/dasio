// SPDX-License-Identifier: GPL-3.0-or-later

//! dante-asio's device process (plan Task 7): one instance's Dante device
//! (the stack copied from iemmixer) whose pump hands each block to the
//! instance's ASIO driver through the shared memory ([`bridge`]), the
//! driver's registration ([`register`]) and the latencies the driver
//! reports ([`latencies`]), run as a Windows service ([`service`]).

pub mod bridge;
#[cfg(windows)]
pub mod jitter;
pub mod logfile;
#[cfg(windows)]
pub mod probe;
pub mod register;
#[cfg(windows)]
pub mod run;
pub mod service;

use dstack::clock::latency_samples;
use dstack::device::DeviceConfig;
use dstack::stream::Timing;

/// The pump's timing on `cfg`: B = its block, P = its latency, W = its
/// work margin (the DAW's time to return a block).
pub fn timing(cfg: &DeviceConfig) -> Timing {
    Timing::of(cfg)
}

/// The DAW's input and output latency, samples: a block reaches the DAW
/// B + P after its first input sample was stamped; its outputs leave with
/// the next block (one B, the double buffer), then W and the advertised TX
/// latency (receivers play our flows at that latency).
pub fn latencies(cfg: &DeviceConfig) -> (u32, u32) {
    let t = timing(cfg);
    let tx = latency_samples(cfg.tx_latency_ns.unwrap_or(cfg.latency_ns), cfg.sample_rate);
    (
        (t.block + t.playout) as u32,
        (t.block + t.margin + tx) as u32,
    )
}

/// The CPUs the device's media threads are pinned to, as an affinity mask
/// (CPUs 0 to 63): the process must hold them, or pinning a thread there
/// fails (Windows error 87).
pub fn own_cpus(cfg: &DeviceConfig) -> usize {
    [cfg.pump_cpu, cfg.io_cpu, cfg.rx_cpu]
        .into_iter()
        .flatten()
        .filter(|&c| c < 64)
        .fold(0, |m, c| m | 1 << c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = include_str!("../../../config/test-dante-device.toml");

    #[test]
    fn latencies_follow_the_device_s_timing() {
        let mut cfg = DeviceConfig::from_toml(DEVICE).unwrap();
        cfg.block = 32;
        cfg.latency_ns = 1_000_000;
        cfg.work_margin = 64;
        cfg.tx_latency_ns = Some(250_000);
        let t = timing(&cfg);
        assert_eq!((t.block, t.playout, t.margin), (32, 96, 64));
        assert_eq!(latencies(&cfg), (32 + 96, 32 + 64 + 24));
    }

    #[test]
    fn a_latency_in_ns_is_its_nearest_whole_samples_as_the_rx_plays_it() {
        // 333 333 ns at 96 kHz is 32 samples (the RX's playout point, the
        // stack's latency_samples); a floor would read 31 and report the
        // DAW one sample short.
        let mut cfg = DeviceConfig::from_toml(DEVICE).unwrap();
        cfg.block = 32;
        cfg.latency_ns = 333_333;
        cfg.work_margin = 64;
        cfg.tx_latency_ns = Some(666_666);
        assert_eq!(timing(&cfg), Timing::of(&cfg));
        assert_eq!(timing(&cfg).playout, 32);
        assert_eq!(latencies(&cfg), (32 + 32, 32 + 64 + 64));
    }

    #[test]
    fn own_cpus_hold_every_pinned_media_thread() {
        let mut cfg = DeviceConfig::from_toml(DEVICE).unwrap();
        assert_eq!(own_cpus(&cfg), 0);
        cfg.pump_cpu = Some(31);
        cfg.io_cpu = Some(29);
        assert_eq!(own_cpus(&cfg), 1 << 31 | 1 << 29);
        // The RX thread on a CPU of its own (gen3 step 2: TX and RX apart).
        cfg.rx_cpu = Some(28);
        assert_eq!(own_cpus(&cfg), 1 << 31 | 1 << 29 | 1 << 28);
        // RX and TX on one CPU (the stack's default): one bit.
        cfg.rx_cpu = Some(29);
        assert_eq!(own_cpus(&cfg), 1 << 31 | 1 << 29);
        // Past the mask's 64 bits: left to the stack's own refusal.
        cfg.rx_cpu = Some(64);
        assert_eq!(own_cpus(&cfg), 1 << 31 | 1 << 29);
    }
}
