// SPDX-License-Identifier: GPL-3.0-or-later

//! dasio's device process (plan Task 7): one instance's Dante device
//! (the stack copied from iemmixer) whose pump hands each block to the
//! instance's ASIO driver through the shared memory ([`bridge`]), the
//! driver's registration ([`register`]) and the latencies the driver
//! reports ([`latencies`]), run as a Windows service ([`service`]).

pub mod app_status;
pub mod bridge;
#[cfg(windows)]
pub mod jitter;
pub mod kernel_tx;
pub mod logfile;
#[cfg(windows)]
pub mod probe;
pub mod register;
#[cfg(windows)]
pub mod run;
pub mod service;
pub mod turnaround;

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
/// latency (receivers play our flows at that latency), plus `spare`
/// blocks the DAW gets beyond the first (see [`bridge::Bridge::with_spare`]).
pub fn latencies(cfg: &DeviceConfig, spare: u32) -> (u32, u32) {
    let t = timing(cfg);
    let tx = latency_samples(cfg.tx_latency_ns.unwrap_or(cfg.latency_ns), cfg.sample_rate);
    (
        (t.block + t.playout) as u32,
        (t.block + t.margin + tx) as u32 + spare * t.block as u32,
    )
}

/// What the pump keeps for its own work after a block's answer is in: the
/// bridge waits for a missing answer until this long before the block's
/// outputs are due at the TX (its process call's due time plus W).
pub const WAIT_SAFETY_NS: u64 = 200_000;

/// How long after a block's start the bridge may wait for the DAW's
/// answer, samples: its due time (B + P after its start) plus the work
/// margin W, less [`WAIT_SAFETY_NS`]; never before the due time.
pub fn wait_offset(t: &Timing, sample_rate: u32) -> u64 {
    let safety = latency_samples(WAIT_SAFETY_NS, sample_rate);
    (t.block + t.playout + t.margin)
        .saturating_sub(safety)
        .max(t.block + t.playout)
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

/// The CPUs the process runs on: every L3 group (an AMD CCD) that holds one
/// of `own`'s CPUs, whole, so the device's other threads (runtime, mDNS,
/// log) run on the media threads' chiplet and never beside other
/// processes' audio on the other one. `l3` is each L3 cache's CPU mask.
/// With no L3 information, `own` plus `fallback` (the CPUs it started
/// with). A process set this way is also one Process Lasso treats as
/// customised and leaves alone. 0 when `own` is 0 (nothing pinned).
pub fn process_cpus(own: usize, l3: &[usize], fallback: usize) -> usize {
    if own == 0 {
        return 0;
    }
    if l3.is_empty() {
        return own | fallback;
    }
    l3.iter()
        .filter(|&&group| group & own != 0)
        .fold(own, |m, &group| m | group)
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
        assert_eq!(latencies(&cfg, 0), (32 + 96, 32 + 64 + 24));
        // A spare block: the outputs play one block later.
        assert_eq!(latencies(&cfg, 1), (32 + 96, 32 + 64 + 24 + 32));
    }

    #[test]
    fn the_bridge_waits_into_the_work_margin_less_the_safety() {
        // FOH master: B 128, P 96 (1 ms), W 128 at 96 kHz; 200 us is 19
        // samples.
        let t = Timing {
            block: 128,
            playout: 96,
            margin: 128,
        };
        assert_eq!(wait_offset(&t, 96_000), 128 + 96 + 128 - 19);
        // A margin smaller than the safety: never before the due time.
        let t = Timing {
            block: 128,
            playout: 96,
            margin: 8,
        };
        assert_eq!(wait_offset(&t, 96_000), 128 + 96);
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
        assert_eq!(latencies(&cfg, 0), (32 + 32, 32 + 64 + 64));
    }

    #[test]
    fn the_process_runs_on_the_ccds_of_its_media_threads() {
        // FOH's 7950X: CCD0 = CPUs 0-15, CCD1 = 16-31.
        let l3 = [0x0000_FFFF, 0xFFFF_0000];
        // Media threads on CCD1: the whole CCD1, nothing of CCD0.
        assert_eq!(
            process_cpus(1 << 31 | 1 << 29 | 1 << 30, &l3, usize::MAX),
            0xFFFF_0000
        );
        // On CCD0: the whole CCD0.
        assert_eq!(
            process_cpus(1 << 13 | 1 << 15, &l3, usize::MAX),
            0x0000_FFFF
        );
        // Threads on both: both.
        assert_eq!(process_cpus(1 << 1 | 1 << 17, &l3, usize::MAX), 0xFFFF_FFFF);
        // No L3 information: own plus the CPUs it started with.
        assert_eq!(process_cpus(1 << 3, &[], 0xF0), 0xF8);
        // An own CPU among the started ones stays in.
        assert_eq!(process_cpus(1 << 4, &[], 0xF0), 0xF0);
        // Nothing pinned: no change asked.
        assert_eq!(process_cpus(0, &l3, usize::MAX), 0);
        // An own CPU outside every listed L3 group still counts.
        assert_eq!(process_cpus(1 << 40, &l3, 0), 1 << 40);
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
