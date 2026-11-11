//! The whole path as on the PC, on a GitHub-hosted Windows runner: the real
//! `dasio` device processes (on the runner's routed IPv4, the stack's
//! test-only system clock: the runner has no dantesync) and an ASIO host on
//! each instance's driver in this process (the probe, as a DAW drives it).
//! Two devices share the runner's one address, as several instances share
//! the PC's: the first on the stack's default ports, the second on its own
//! ports and process id. In each, RX 1 is subscribed to its own TX 1
//! through the device's state file, so each impulse the host sends on
//! output 1 goes out through Dante and comes back on input 1. Only our own
//! devices are subscribed.
#![cfg(windows)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dstack::device::{DeviceConfig, DeviceState, Subscription};

const TEST_DEVICE: &str = include_str!("../../../config/test-dante-device.toml");
/// The instances the test runs (the driver's host tests use 4 to 12 and 16,
/// its conformance test 3, the service test 14).
const FIRST: u32 = 13;
const SECOND: u32 = 15;
/// The second device's ARC, CMC and flow-control ports and its process id
/// (the first keeps the stack's 4440, 8800, 4455 and 0).
const SECOND_PORTS: &str = "arc_port = 4441\ncmc_port = 8801\nflows_port = 4456\nprocess_id = 1\n";
/// The first device's pump on a CPU of its own and its TX and RX on
/// another, as the band instance has its own CPUs on FOH (pump, rx, io).
/// Unpinned, its spinning pump lost its core while the second ran (CI
/// 42774079830 and 42820731724: 8442 and 10078 switches of 12000, the pump
/// missing up to 958 blocks a second, dasio#28). With only the pump pinned,
/// its own spinning TX or RX could still land on the pump's CPU (CI
/// 42944603438: the device alone lost up to 1137 blocks a second). Both
/// devices' TX and RX on one CPU left the second receiving nothing (CI
/// 42822347591): the second runs on the normal profile, unpinned, and takes
/// none of these two CPUs' spinning.
const FIRST_CPUS: &str = "pump_cpu = 1\nio_cpu = 2\n";
/// The second device on the "normal" timing profile, as mbc runs: no spin,
/// no real-time priority, timers for the whole wait, a 2 ms work margin
/// (192 samples). Two low-latency devices spin a pump and a TX thread each,
/// more spinners than a 4-CPU runner has CPUs: with both on it, each pump
/// lost up to 4083 blocks a second (CI 42891517726, 42896309664: 1 run in
/// 4 failed, dasio#28). This test is about two devices on one address; the
/// first device's real-time timing is checked alone, before the second
/// starts.
const SECOND_TIMING: &str = "timing = \"normal\"\nwork_margin = 192\n";
/// The CPUs the test needs: the first device's pump (1), its TX and RX
/// (2), the host and the second device on the rest.
const CPUS_NEEDED: usize = 4;
/// How long a device may take to publish its shared memory.
const READY: Duration = Duration::from_secs(20);
/// How long a device may take to end after its stop.
const STOP: Duration = Duration::from_secs(15);

/// The test device's config in `dir` under `name`, with `extra` keys, at a
/// 10 ms latency (a shared runner schedules a normal-priority thread over
/// 1 ms late now and then: the path is under test here, real-time timing
/// on the PC), with a state file that subscribes RX 1 to its own TX 1.
fn device_in(dir: &Path, name: &str, extra: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let state = dir.join("state.toml");
    let text = format!(
        "{}\n{extra}state_file = {:?}\n",
        TEST_DEVICE
            .lines()
            .filter(|l| !l.starts_with("state_file"))
            .map(|l| {
                if l.starts_with("latency_ns") {
                    "latency_ns = 10000000".to_string()
                } else if l.starts_with("name ") {
                    format!("name = {name:?}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        state.display().to_string()
    );
    let path = dir.join("device.toml");
    std::fs::write(&path, &text).unwrap();
    let cfg = DeviceConfig::from_toml(&text).unwrap();
    let mut saved = DeviceState::from_config(&cfg);
    saved.subscriptions = vec![None; cfg.rx.len()];
    saved.subscriptions[0] = Some(Subscription {
        tx_channel: cfg.tx[0].clone(),
        tx_device: cfg.name.clone(),
    });
    saved.save(&state).unwrap();
    path
}

/// The runner's IPv4 address on its default route (connecting a UDP
/// socket picks the route and sends nothing).
fn routed_ipv4() -> Ipv4Addr {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).unwrap();
    match probe.local_addr().unwrap() {
        SocketAddr::V4(a) if !a.ip().is_loopback() && !a.ip().is_unspecified() => *a.ip(),
        other => panic!("no routed IPv4 address on this runner: {other}"),
    }
}

/// A device process; killed if the test ends without stopping it (a
/// failed assertion), so it never outlives the test.
struct Device {
    instance: u32,
    child: Option<Child>,
    log: PathBuf,
}

impl Device {
    /// `dasio run` for `instance` with `config` on `ip`, once its
    /// shared memory is up.
    fn start(instance: u32, config: &Path, ip: Ipv4Addr, log: PathBuf) -> Device {
        let child = Command::new(exe())
            .args(["run", "--instance", &instance.to_string(), "--config"])
            .arg(config)
            .args(["--ip", &ip.to_string(), "--clock", "system", "--log"])
            .arg(&log)
            .stdin(Stdio::null())
            .spawn()
            .expect("the device process");
        let mut device = Device {
            instance,
            child: Some(child),
            log,
        };
        let t = Instant::now();
        while shm::Shared::open(instance).is_err() {
            if let Some(status) = device.child.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "instance {instance} ended at the start ({status}):\n{}",
                    device.log()
                );
            }
            assert!(
                t.elapsed() < READY,
                "instance {instance}: no shared memory after {READY:?}:\n{}",
                device.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        device
    }

    /// The process still runs (a device that ended on its own fails).
    fn assert_running(&mut self) {
        if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
            panic!(
                "instance {} ended on its own ({status}):\n{}",
                self.instance,
                self.log()
            );
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// A stop as `install.ps1` and the service ask it: the device ends
    /// cleanly.
    fn stop(mut self) {
        let instance = self.instance;
        let status = Command::new(exe())
            .args(["stop", "--instance", &instance.to_string()])
            .status()
            .unwrap();
        assert!(status.success(), "dasio stop {instance}: {status}");
        let mut child = self.child.take().unwrap();
        let t = Instant::now();
        let ended = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            if t.elapsed() > STOP {
                let _ = child.kill();
                panic!(
                    "instance {instance} did not end within {STOP:?}:\n{}",
                    self.log()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            ended.success(),
            "instance {instance}'s exit: {ended}\n{}",
            self.log()
        );
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            eprintln!("instance {}'s log:\n{}", self.instance, self.log());
        }
    }
}

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_dasio")
}

/// The host on `instance` for `seconds`: the device drives it, and each
/// impulse comes back once over Dante, at one fixed round trip within the
/// latencies the driver reports. Returns the report for the timing checks.
fn round_trip(instance: u32, seconds: u64) -> dasio::probe::Report {
    round_trip_stalled(instance, seconds, 0)
}

/// [`round_trip`] with the host stalling once for `stall_ms` while an
/// impulse is on its way.
fn round_trip_stalled(instance: u32, seconds: u64, stall_ms: u64) -> dasio::probe::Report {
    let r = dasio::probe::probe_stalled(instance, seconds, stall_ms).expect("the probe");
    eprintln!("instance {instance}: {r:?}");
    let block = u64::from(r.block);
    // This probe's own switches only, not an earlier probe's in this
    // process too.
    assert!(
        r.switches <= seconds * 96_000 / block * 11 / 10,
        "instance {instance}: {} switches in {seconds} s at block {block}: more than the device drove",
        r.switches
    );
    assert!(
        r.switches >= seconds * 96_000 / block * 9 / 10,
        "instance {instance}: the device drives the host: {} switches in {seconds} s at block {block}",
        r.switches
    );
    // One impulse at each whole second inside the samples the host's
    // timeline ran (the impulse at 1 s the first): a probe a few blocks
    // short of 4 s sends 3 (CI 41375343538: 11996 switches of 32, 3
    // impulses). A second the timeline jumped past sends none: the host
    // missed its block, or the device's pump skipped it, which the
    // timeline keeps since it runs on media time (CI 41415967913: 3 sent
    // in 385120 samples, no block missed, one second jumped).
    let whole_seconds = r.samples.saturating_sub(1) / 96_000;
    assert_eq!(
        r.emitted + r.skipped,
        whole_seconds,
        "instance {instance}: {} impulses sent and {} skipped in {} samples ({} blocks missed)",
        r.emitted,
        r.skipped,
        r.samples,
        r.missed
    );
    assert!(
        r.emitted + r.skipped + 1 >= seconds,
        "instance {instance}: impulses sent: {} and skipped: {} in {seconds} s",
        r.emitted,
        r.skipped
    );
    assert!(
        r.round_trips.len() as u64 >= r.emitted - 1,
        "instance {instance}: impulses back: {} of {}",
        r.round_trips.len(),
        r.emitted
    );
    let (min, max) = (
        *r.round_trips.iter().min().unwrap(),
        *r.round_trips.iter().max().unwrap(),
    );
    assert_eq!(
        min, max,
        "instance {instance}: one fixed round trip: {:?}",
        r.round_trips
    );
    // Out (the driver's output latency) and back in (its input latency):
    // the round trip the driver reports to the DAW is what the path takes.
    assert!(
        min <= (r.latency_in + r.latency_out) as u64 + block,
        "instance {instance}: round trip {min} against reported {} + {}",
        r.latency_in,
        r.latency_out
    );
    r
}

#[test]
fn an_impulse_goes_out_through_dante_and_comes_back_on_two_devices_sharing_one_address() {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert!(
        cpus >= CPUS_NEEDED,
        "{cpus} CPUs: the two devices' pinned threads need {CPUS_NEEDED}"
    );
    let dir = std::env::temp_dir().join(format!("dasio-e2e-{}", std::process::id()));
    let ip = routed_ipv4();
    let first = Device::start(
        FIRST,
        &device_in(&dir.join("first"), "test-device", FIRST_CPUS),
        ip,
        dir.join("first.log"),
    );
    // A moment for the flows to be granted.
    std::thread::sleep(Duration::from_secs(2));

    // The host at High priority, as Ableton runs (and `dasio probe`).
    // Timed with the first device alone: two devices spinning on a 4-CPU
    // runner still share its CPUs beyond their pumps (31.10.2026: 2 late
    // blocks with no host gap); on the PC each instance has its own CPUs,
    // and that is the PC's gate (HIL, soak).
    dasio::probe::high_priority().expect("High priority");
    let r = round_trip(FIRST, 6);
    // A late block is the device's fault unless the host itself lost its
    // CPU: a shared runner's VM deschedules a thread now and then (a 48.9
    // ms gap, CI 41130881044; a 763 us one at block 32, CI 41340984953,
    // past the 667 us work margin). Each host gap over 1.5 blocks may cost
    // at most the blocks it spans; any other late block fails (the attach
    // defect b0233cd fixed read late 3 with no gap). Real-time timing is
    // the FOH PC's gate (HIL, soak).
    assert!(
        r.late <= r.gap_blocks,
        "{} late blocks, {} explained by the host's gaps (longest {} us, {} over 1 ms)",
        r.late,
        r.gap_blocks,
        r.max_gap_us,
        r.gaps_over_1ms
    );

    // A host that loses its CPU while an impulse is on its way (CI
    // 41375781285: a 57 ms runner gap, one round trip of 640 among 1120):
    // the driver's sample position counts the blocks it missed, as a sound
    // card's does, so the DAW's timeline keeps the round trip it reports.
    let r = round_trip_stalled(FIRST, 4, 5);
    assert!(
        r.missed > 0,
        "the 5 ms host stall missed no block (max gap {} us)",
        r.max_gap_us
    );

    // The second device on the same address, beside the running first:
    // on its own ports (ARC, CMC and flow control, each held by one of
    // them, so this process cannot take it).
    let mut second = Device::start(
        SECOND,
        &device_in(
            &dir.join("second"),
            "test-device-2",
            &format!("{SECOND_PORTS}{SECOND_TIMING}"),
        ),
        ip,
        dir.join("second.log"),
    );
    std::thread::sleep(Duration::from_secs(2));
    for port in [4440, 8800, 4455, 4441, 8801, 4456] {
        assert!(
            UdpSocket::bind((ip, port)).is_err(),
            "UDP {port} on {ip} is free: a device does not hold it"
        );
    }
    // Both carry audio, each its own.
    round_trip(SECOND, 4);
    round_trip(FIRST, 4);

    // The first device manages the address (it bound 8700 first). Its
    // stop must not end the second: it keeps its audio and takes the
    // address over (1.11.2026, iem-snv: the member ended with os error
    // 10054 when the manager stopped, and the SCM restarted it).
    first.stop();
    std::thread::sleep(Duration::from_secs(4));
    second.assert_running();
    round_trip(SECOND, 4);
    second.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
