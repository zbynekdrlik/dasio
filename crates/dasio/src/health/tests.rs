use super::*;
use std::io::Write;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use dstack::device::DeviceConfig;
use dstack::media::ring::Ring;
use dstack::media::rx::RxShared;
use dstack::media::tx::TxShared;
use dstack::stream::{Map, Pump, PumpStats, Timing};
use dstack_audio_io::{Block, Process};

/// A processor that panics in its first block, as a bug in the bridge would.
struct Panics;

impl Process for Panics {
    fn process(&mut self, _: &mut Block<'_>) {
        panic!("a test panic in the bridge");
    }
}

/// A processor that does nothing.
struct Quiet;

impl Process for Quiet {
    fn process(&mut self, _: &mut Block<'_>) {}
}

/// dstack's pump of the test device around `processor`, as the service
/// builds it (on the loopback address), and its statistics.
fn pump_of<P: Process>(processor: P) -> (Pump<P>, Arc<PumpStats>) {
    let cfg = DeviceConfig::from_toml(include_str!("../../../../config/test-dante-device.toml"))
        .expect("the test device's config");
    let rings: Arc<[Ring]> = (0..cfg.rx.len())
        .map(|_| Ring::new())
        .collect::<Vec<_>>()
        .into();
    let rx = Arc::new(
        RxShared::new(cfg.sample_rate, Ipv4Addr::LOCALHOST, Arc::clone(&rings))
            .expect("the RX side"),
    );
    let engine: Vec<usize> = (0..cfg.tx.len()).collect();
    let tx = Arc::new(
        TxShared::new(cfg.clone(), Ipv4Addr::LOCALHOST, rings, 1, &engine).expect("the TX side"),
    );
    let timing = Timing::of(&cfg);
    let stats = Arc::new(PumpStats::new(timing, cfg.sample_rate));
    let map = Map {
        rx: (0..cfg.rx.len()).collect(),
        tx: engine,
    };
    let pump = Pump::new(
        processor,
        map,
        timing,
        cfg.encoding,
        rx,
        tx,
        Arc::clone(&stats),
    );
    (pump, stats)
}

#[test]
fn a_bridge_that_panicked_ends_the_device() {
    let (mut pump, stats) = pump_of(Panics);
    pump.free();
    let why = pump_failure(&stats.stream_stats(), false).expect("the device must end");
    assert!(
        why.contains("a test panic in the bridge"),
        "the reason names the panic: {why}"
    );
}

#[test]
fn a_running_pump_keeps_the_device() {
    let (mut pump, stats) = pump_of(Quiet);
    pump.free();
    pump.free();
    assert_eq!(pump_failure(&stats.stream_stats(), false), None);
}

#[test]
fn an_ended_pump_thread_ends_the_device() {
    let (mut pump, stats) = pump_of(Quiet);
    pump.free();
    let why = pump_failure(&stats.stream_stats(), true).expect("the device must end");
    assert!(why.contains("pump"), "the reason names the pump: {why}");
}

/// The log as a test reads it.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A panic, as a bug in any of the device's threads would raise.
fn panics_in_the_device() {
    panic!("a test panic in the device");
}

#[test]
fn a_panic_is_written_to_the_log() {
    let log = Log::default();
    let to = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || to.clone())
        .finish();
    log_panics();
    tracing::subscriber::with_default(subscriber, || {
        let caught = std::panic::catch_unwind(panics_in_the_device);
        assert!(caught.is_err());
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("ERROR"), "at ERROR: {text}");
    assert!(
        text.contains("a test panic in the device"),
        "its message: {text}"
    );
    assert!(text.contains("health"), "its place: {text}");
    // libtest names the thread after the test, or "main" with one thread.
    assert!(text.contains("thread="), "its thread: {text}");
}
