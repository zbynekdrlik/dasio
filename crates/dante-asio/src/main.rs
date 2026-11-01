// SPDX-License-Identifier: GPL-3.0-or-later

//! `dante-asio run --instance <n> --config <device.toml> --ip <address>`:
//! one instance's Dante device and its shared memory for the driver;
//! `dante-asio service ...` (same flags): the same, as the SCM starts it.
//! `dante-asio register --instance <n> --dll <path> [--name <shown to DAWs>]` and
//! `dante-asio unregister --instance <n>`: the driver's registry entries
//! (administrator).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: dante-asio run --instance <n> --config <device.toml> --ip <address> [--log <file>] [--clock system]\n       \
         dante-asio service --instance <n> --config <device.toml> --ip <address> --log <file> [--clock system]\n       \
         dante-asio register --instance <n> --dll <path> [--name <shown to DAWs>]\n       \
         dante-asio stop --instance <n>\n       \
         dante-asio probe --instance <n> [--seconds <s>] [--affinity <hex mask>]\n       \
         dante-asio jitter --cpus <n,n,..> --seconds <s>\n       \
         dante-asio unregister --instance <n> [--name <shown to DAWs>]"
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
        dante_asio::logfile::RotatingLog::open(
            std::path::Path::new(&p),
            dante_asio::logfile::MAX_BYTES,
            dante_asio::logfile::KEEP,
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
            eprintln!("dante-asio: the log file: {e}");
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
                eprintln!("dante-asio: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let Some(instance) = flag(&args, "--instance").and_then(|n| n.parse::<u32>().ok()) else {
        return usage();
    };
    let result: Result<(), String> = match cmd.as_str() {
        "register" => match flag(&args, "--dll").and_then(|dll| {
            dante_asio::register::entries(instance, &dll, flag(&args, "--name").as_deref())
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
        "unregister" => {
            match dante_asio::register::keys(instance, flag(&args, "--name").as_deref()) {
                Some(keys) => remove(&keys),
                None => return usage(),
            }
        }
        "run" | "service" => {
            let (Some(config), Some(ip)) = (
                flag(&args, "--config"),
                flag(&args, "--ip").and_then(|ip| ip.parse().ok()),
            ) else {
                return usage();
            };
            // `--clock system`: a test machine's own clock (CI has no
            // dantesync); never on a real Dante network.
            let system_clock = match flag(&args, "--clock").as_deref() {
                None | Some("dantesync") => false,
                Some("system") => true,
                Some(_) => return usage(),
            };
            if cmd == "service" {
                service(instance, config.into(), ip, system_clock)
            } else {
                run(instance, config.into(), ip, system_clock)
            }
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dante-asio: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn apply(entries: &[dante_asio::register::Entry]) -> Result<(), String> {
    dante_asio::register::apply(entries).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn remove(keys: &[String]) -> Result<(), String> {
    dante_asio::register::remove(keys).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn stop(instance: u32) -> Result<(), String> {
    match dante_asio::run::ask_stop(instance) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(windows)]
fn probe(instance: u32, seconds: u64, affinity: Option<usize>) -> Result<(), String> {
    if let Some(mask) = affinity {
        dante_asio::probe::set_affinity(mask)?;
    }
    dante_asio::probe::high_priority()?;
    let r = dante_asio::probe::probe(instance, seconds)?;
    let min = r.round_trips.iter().min();
    let max = r.round_trips.iter().max();
    println!(
        "probe instance {instance}: block {} latency_in {} latency_out {} switches {} emitted {} arrived {} round_trip_min {:?} round_trip_max {:?} late {} max_gap_us {} gaps_over_1ms {} gap_blocks {} max_switch_us {}",
        r.block,
        r.latency_in,
        r.latency_out,
        r.switches,
        r.emitted,
        r.round_trips.len(),
        min,
        max,
        r.late,
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
) -> Result<(), String> {
    let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    dante_asio::run::run(instance, config, ip, clock(system_clock), never)
}

#[cfg(windows)]
fn clock(system: bool) -> iem_dante::host::ClockSource {
    if system {
        iem_dante::host::ClockSource::System
    } else {
        iem_dante::host::ClockSource::Dantesync
    }
}

#[cfg(windows)]
fn service(
    instance: u32,
    config: std::path::PathBuf,
    ip: std::net::Ipv4Addr,
    system_clock: bool,
) -> Result<(), String> {
    dante_asio::service::windows::dispatch(instance, config, ip, clock(system_clock))
}

#[cfg(not(windows))]
fn service(
    _instance: u32,
    _config: std::path::PathBuf,
    _ip: std::net::Ipv4Addr,
    _system_clock: bool,
) -> Result<(), String> {
    Err("Windows only".into())
}

#[cfg(not(windows))]
fn apply(_entries: &[dante_asio::register::Entry]) -> Result<(), String> {
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
        let j = dante_asio::jitter::jitter(cpu, seconds)?;
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
