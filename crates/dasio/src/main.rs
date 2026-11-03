// SPDX-License-Identifier: GPL-3.0-or-later

//! `dasio run --instance <n> --config <device.toml> --ip <address>`:
//! one instance's Dante device and its shared memory for the driver;
//! `dasio service ...` (same flags): the same, as the SCM starts it.
//! `dasio register --instance <n> --dll <path> [--name <shown to DAWs>]` and
//! `dasio unregister --instance <n>`: the driver's registry entries
//! (administrator).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: dasio run --instance <n> --config <device.toml> --ip <address> [--log <file>] [--clock system] [--spare-blocks <n>]\n       \
         dasio service --instance <n> --config <device.toml> --ip <address> --log <file> [--clock system] [--spare-blocks <n>]\n       \
         dasio register --instance <n> --dll <path> [--name <shown to DAWs>]\n       \
         dasio stop --instance <n>\n       \
         dasio probe --instance <n> [--seconds <s>] [--affinity <hex mask>]\n       \
         dasio jitter --cpus <n,n,..> --seconds <s>\n       \
         dasio unregister --instance <n> [--name <shown to DAWs>]"
    );
    ExitCode::from(2)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter =
        || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    // `--log <path>`: appended to that file (the service has no console),
    // bounded in size (old files beside it).
    match flag(&args, "--log").map(|p| {
        dasio::logfile::RotatingLog::open(
            std::path::Path::new(&p),
            dasio::logfile::MAX_BYTES,
            dasio::logfile::KEEP,
        )
    }) {
        Some(Ok(file)) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter())
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init();
            // Logged to a file: no console window on the PC's screen (the
            // logon task's console stood over the FOH Ableton).
            hide_console();
        }
        Some(Err(e)) => {
            eprintln!("dasio: the log file: {e}");
            return ExitCode::FAILURE;
        }
        None => tracing_subscriber::fmt().with_env_filter(filter()).init(),
    }
    let Some(cmd) = args.first() else {
        return usage();
    };
    if cmd == "jitter" {
        let (Some(cpus), Some(seconds)) = (
            flag(&args, "--cpus"),
            flag(&args, "--seconds").and_then(|s| s.parse::<u64>().ok()),
        ) else {
            return usage();
        };
        return match jitter(&cpus, seconds) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("dasio: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let Some(instance) = flag(&args, "--instance").and_then(|n| n.parse::<u32>().ok()) else {
        return usage();
    };
    let result: Result<(), String> = match cmd.as_str() {
        "register" => match flag(&args, "--dll").and_then(|dll| {
            dasio::register::entries(instance, &dll, flag(&args, "--name").as_deref())
        }) {
            Some(entries) => apply(&entries),
            None => return usage(),
        },
        "stop" => stop(instance),
        "probe" => {
            let seconds = flag(&args, "--seconds")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            // `--affinity <hex mask>`: the probe's own CPUs (a DAW's worker
            // runs in the DAW's set, not on the device's cores).
            let affinity = flag(&args, "--affinity")
                .and_then(|m| usize::from_str_radix(m.trim_start_matches("0x"), 16).ok());
            probe(instance, seconds, affinity)
        }
        "unregister" => match dasio::register::keys(instance, flag(&args, "--name").as_deref()) {
            Some(keys) => remove(&keys),
            None => return usage(),
        },
        "run" | "service" => {
            let (Some(config), Some(ip)) = (
                flag(&args, "--config"),
                flag(&args, "--ip").and_then(|ip| ip.parse().ok()),
            ) else {
                return usage();
            };
            // `--clock system`: a test machine's own clock (CI has no
            // dsync); never on a real Dante network.
            let system_clock = match flag(&args, "--clock").as_deref() {
                None | Some("dsync") => false,
                Some("system") => true,
                Some(_) => return usage(),
            };
            // `--spare-blocks <n>`: block periods the DAW gets beyond the
            // first (a heavy set's late blocks), its outputs that much later.
            let spare = match flag(&args, "--spare-blocks").map(|n| n.parse::<u32>()) {
                None => 0,
                Some(Ok(n)) if n <= 1 => n,
                Some(_) => return usage(),
            };
            if cmd == "service" {
                service(instance, config.into(), ip, system_clock, spare)
            } else {
                run(instance, config.into(), ip, system_clock, spare)
            }
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dasio: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn apply(entries: &[dasio::register::Entry]) -> Result<(), String> {
    dasio::register::apply(entries).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn remove(keys: &[String]) -> Result<(), String> {
    dasio::register::remove(keys).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn stop(instance: u32) -> Result<(), String> {
    match dasio::run::ask_stop(instance) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(windows)]
fn probe(instance: u32, seconds: u64, affinity: Option<usize>) -> Result<(), String> {
    if let Some(mask) = affinity {
        dasio::probe::set_affinity(mask)?;
    }
    dasio::probe::high_priority()?;
    let r = dasio::probe::probe(instance, seconds)?;
    let min = r.round_trips.iter().min();
    let max = r.round_trips.iter().max();
    println!(
        "probe instance {instance}: block {} latency_in {} latency_out {} switches {} emitted {} arrived {} round_trip_min {:?} round_trip_max {:?} late {} missed {} samples {} max_gap_us {} gaps_over_1ms {} gap_blocks {} max_switch_us {}",
        r.block,
        r.latency_in,
        r.latency_out,
        r.switches,
        r.emitted,
        r.round_trips.len(),
        min,
        max,
        r.late,
        r.missed,
        r.samples,
        r.max_gap_us,
        r.gaps_over_1ms,
        r.gap_blocks,
        r.max_switch_us
    );
    for (second, (peak, at)) in r.rx_peaks.iter().enumerate() {
        println!("rx1 second {second}: peak {peak} at sample {at}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn probe(_instance: u32, _seconds: u64, _affinity: Option<usize>) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn stop(_instance: u32) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(windows)]
fn run(
    instance: u32,
    config: std::path::PathBuf,
    ip: std::net::Ipv4Addr,
    system_clock: bool,
    spare: u32,
) -> Result<(), String> {
    let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    dasio::run::run(instance, config, ip, clock(system_clock), spare, never)
}

#[cfg(windows)]
fn clock(system: bool) -> dstack::host::ClockSource {
    if system {
        dstack::host::ClockSource::System
    } else {
        dstack::host::ClockSource::Dsync
    }
}

#[cfg(windows)]
fn service(
    instance: u32,
    config: std::path::PathBuf,
    ip: std::net::Ipv4Addr,
    system_clock: bool,
    spare: u32,
) -> Result<(), String> {
    dasio::service::windows::dispatch(instance, config, ip, clock(system_clock), spare)
}

#[cfg(not(windows))]
fn service(
    _instance: u32,
    _config: std::path::PathBuf,
    _ip: std::net::Ipv4Addr,
    _system_clock: bool,
    _spare: u32,
) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn apply(_entries: &[dasio::register::Entry]) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn remove(_keys: &[String]) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn run(
    _instance: u32,
    _config: std::path::PathBuf,
    _ip: std::net::Ipv4Addr,
    _system_clock: bool,
    _spare: u32,
) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(windows)]
fn hide_console() {
    // SAFETY: detaches this process from its console; its window closes
    // when no other process holds it.
    unsafe { windows_sys::Win32::System::Console::FreeConsole() };
}

#[cfg(not(windows))]
fn hide_console() {}

/// `--cpus 14,15,0`: one run per CPU, one line each.
#[cfg(windows)]
fn jitter(cpus: &str, seconds: u64) -> Result<(), String> {
    for c in cpus.split(',') {
        let cpu: usize = c.trim().parse().map_err(|_| format!("not a CPU: {c}"))?;
        let j = dasio::jitter::jitter(cpu, seconds)?;
        println!(
            "jitter cpu {}: over_50us {} over_500us {} over_2ms {} longest_us {}",
            j.cpu, j.over_50us, j.over_500us, j.over_2ms, j.longest_us
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn jitter(_cpus: &str, _seconds: u64) -> Result<(), String> {
    Err("Windows only".into())
}
