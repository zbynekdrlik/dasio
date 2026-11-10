// SPDX-License-Identifier: GPL-3.0-or-later

//! dasio's device process (plan Task 7): one instance's Dante device
//! (the stack copied from iemmixer) whose pump hands each block to the
//! instance's ASIO driver through the shared memory ([`bridge`]), the
//! driver's registration ([`register`]) and the latencies the driver
//! reports ([`latencies`]), run as a Windows service ([`service`]).

pub mod app_status;
pub mod bridge;
pub mod bursts;
pub mod cpu_snap;
pub mod health;
#[cfg(windows)]
pub mod jitter;
pub mod kernel_tx;
pub mod logfile;
pub mod minutes;
pub mod pc_profile;
#[cfg(windows)]
pub mod probe;
pub mod register;
#[cfg(windows)]
pub mod run;
pub mod service;
pub mod turnaround;

/// How an instance runs beyond its device config (the service's flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tuning {
    /// Block periods the DAW gets beyond the first (`--spare-blocks`).
    pub spare: u32,
    /// Audio TX through dasio's NDIS filter (`--kernel-tx`, dasio#1),
    /// the UDP sockets where it cannot be opened.
    pub kernel_tx: bool,
}

/// Where the device's PTP time comes from (`--clock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clock {
    /// The default (owner, 4.11.2026): dsync's in-process PTP follower on
    /// the device's interface when it is locked, the dsync service
    /// otherwise. It only reads PTP; dantesync stays the PC's clock owner.
    #[default]
    Linked,
    /// The dsync service only (`--clock dsync`): the fallback if the
    /// in-process follower misbehaves on a PC.
    Dsync,
    /// TEST ONLY (`--clock system`): this machine's own clock (CI).
    System,
}

impl Clock {
    /// The `--clock` flag's value; `None` for an unknown one.
    pub fn parse(flag: Option<&str>) -> Option<Clock> {
        match flag {
            None | Some("linked") => Some(Clock::Linked),
            Some("dsync") => Some(Clock::Dsync),
            Some("system") => Some(Clock::System),
            Some(_) => None,
        }
    }
}

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

/// The CPUs the process's helper threads (everything but the media
/// threads: the main loop, the log, mDNS, the runtime, the profile check)
/// are kept on: the process's CPUs the media threads leave free; 0 when
/// none is free or nothing is pinned. A media thread's spin tail yields
/// its CPU to any ready thread there, and a helper took it for a quantum:
/// TX lanes restarted about once a minute (FOH, dasio#1, 4.11.2026).
pub fn helper_cpus(process: usize, own: usize) -> usize {
    if own == 0 { 0 } else { process & !own }
}

/// The CPUs the PC profile check's child runs on: this process's helper
/// CPUs ([`helper_cpus`]), never the rest of the system, where another
/// instance's media threads are; 0 means it is not moved. Master's child on
/// `system & !own` ran on band's media CPUs and band's TX schedule restarted
/// (00:56:22Z, dasio#1, 5.11.2026).
pub fn child_cpus(process: usize, system: usize, own: usize) -> usize {
    helper_cpus(process, own) & system
}

/// The creation flags of the PC profile check's child: normal priority, no
/// window, and suspended when it must first leave the media CPUs (`own`):
/// started running, it disturbed the media threads before it was moved
/// (band's receivers, 22:59:06Z, dasio#1, 5.11.2026).
pub fn child_creation_flags(own: usize) -> u32 {
    if own == 0 {
        CHILD_FLAGS
    } else {
        CHILD_FLAGS_SUSPENDED
    }
}

/// NORMAL_PRIORITY_CLASS (0x20) and CREATE_NO_WINDOW (0x0800_0000).
const CHILD_FLAGS: u32 = 0x0800_0020;
/// The same and CREATE_SUSPENDED (0x4).
const CHILD_FLAGS_SUSPENDED: u32 = 0x0800_0024;

/// The CPU set ids (Windows' `SYSTEM_CPU_SET_INFORMATION`, as
/// `(group, logical processor index, id)`) of the group-0 CPUs in `mask`:
/// the process's default CPU sets, so every thread it starts begins on the
/// helper CPUs; a media thread's own affinity takes precedence over them.
pub fn cpu_set_ids(mask: usize, sets: &[(u16, u8, u32)]) -> Vec<u32> {
    sets.iter()
        .filter(|&&(group, cpu, _)| {
            group == 0
                && 1usize
                    .checked_shl(u32::from(cpu))
                    .is_some_and(|b| mask & b != 0)
        })
        .map(|&(_, _, id)| id)
        .collect()
}

/// Whether a thread (by its name) is one of dstack's media threads, which
/// pin themselves to their own CPU.
pub fn is_media_thread(name: &str) -> bool {
    matches!(name, "dstack-engine" | "dstack-tx" | "dstack-rx")
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

    #[test]
    fn new_threads_get_the_cpu_set_ids_of_the_helper_cpus_only() {
        use super::cpu_set_ids;
        // (group, logical processor index, CPU set id) as Windows lists
        // them: ids start at 0x100 on FOH's Ryzen.
        let sets: Vec<(u16, u8, u32)> = (0..16).map(|c| (0, c, 0x100 + c as u32)).collect();
        assert_eq!(cpu_set_ids(0x800, &sets), vec![0x10B]);
        assert_eq!(cpu_set_ids(0x8800, &sets), vec![0x10B, 0x10F]);
        assert_eq!(cpu_set_ids(0, &sets), Vec::<u32>::new());
        // Only group 0: the masks name group 0's CPUs.
        let two = vec![(0, 3, 7), (1, 3, 9)];
        assert_eq!(cpu_set_ids(0x8, &two), vec![7]);
    }

    #[test]
    fn the_profile_child_starts_suspended_when_it_must_be_moved_first() {
        use super::child_creation_flags;
        // NORMAL_PRIORITY_CLASS 0x20, CREATE_NO_WINDOW 0x0800_0000,
        // CREATE_SUSPENDED 0x4.
        assert_eq!(child_creation_flags(0x700), 0x0800_0024);
        assert_eq!(child_creation_flags(0), 0x0800_0020);
    }

    #[test]
    fn the_profile_child_stays_off_every_instance_s_media_cpus() {
        use super::child_cpus;
        // Master on FOH: Lasso gives 8-11, its media threads hold 8-10;
        // band's media threads hold 12-14. Master's child ran on band's
        // CPUs and band's TX restarted (00:56:22Z, dasio#1, 5.11.2026).
        assert_eq!(child_cpus(0xF00, 0xFFFF, 0x700), 0x800);
        assert_eq!(child_cpus(0xF000, 0xFFFF, 0x7000), 0x8000);
        // Nothing pinned (mbc): not moved.
        assert_eq!(child_cpus(0xFFF, 0xFFF, 0), 0);
        // No CPU free of the media threads: not moved.
        assert_eq!(child_cpus(0x700, 0xFFFF, 0x700), 0);
    }

    #[test]
    fn helpers_go_to_the_process_s_cpus_the_media_threads_leave_free() {
        // Master on FOH: Lasso gives 8-11, the media threads hold 8-10.
        assert_eq!(super::helper_cpus(0xF00, 0x700), 0x800);
        // Band: 12-15, media 12-14.
        assert_eq!(super::helper_cpus(0xF000, 0x7000), 0x8000);
        // No pins (mbc): nothing to move.
        assert_eq!(super::helper_cpus(0xFFF, 0), 0);
        // The process holds only the media CPUs: nowhere to put them.
        assert_eq!(super::helper_cpus(0x700, 0x700), 0);
    }

    #[test]
    fn only_dstack_s_media_threads_stay_where_they_are() {
        for name in ["dstack-engine", "dstack-tx", "dstack-rx"] {
            assert!(super::is_media_thread(name), "{name}");
        }
        for name in [
            "",
            "main",
            "pc-profile",
            "turnaround",
            "kernel-tx-adapter",
            "mdns-service",
            "tokio-runtime-worker",
            "dstack-device",
        ] {
            assert!(!super::is_media_thread(name), "{name}");
        }
    }
    use super::Clock;

    #[test]
    fn the_clock_flag_defaults_to_the_in_process_follower() {
        assert_eq!(Clock::parse(None), Some(Clock::Linked));
        assert_eq!(Clock::parse(Some("linked")), Some(Clock::Linked));
        assert_eq!(Clock::parse(Some("dsync")), Some(Clock::Dsync));
        assert_eq!(Clock::parse(Some("system")), Some(Clock::System));
        assert_eq!(Clock::parse(Some("ptp")), None);
        assert_eq!(Clock::default(), Clock::Linked);
    }

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
