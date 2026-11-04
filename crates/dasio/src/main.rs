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
        "usage: dasio run --instance <n> --config <device.toml> --ip <address> [--log <file>] [--clock linked|dsync|system] [--spare-blocks <n>] [--kernel-tx]\n       \
         dasio service --instance <n> --config <device.toml> --ip <address> --log <file> [--clock linked|dsync|system] [--spare-blocks <n>] [--kernel-tx]\n       \
         dasio register --instance <n> --dll <path> [--name <shown to DAWs>]\n       \
         dasio stop --instance <n>\n       \
         dasio probe --instance <n> [--seconds <s>] [--affinity <hex mask>]\n       \
         dasio jitter --cpus <n,n,..> --seconds <s>\n       \
         dasio pc-profile check|apply [--profile <file>] [--plan]\n       \
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
    if cmd == "pc-profile" {
        // `dasio pc-profile check|apply [--profile <file>] [--plan]`
        // (dasio#5): the PC's Windows settings against its profile.
        let path =
            flag(&args, "--profile").unwrap_or_else(|| dasio::pc_profile::DEFAULT_PATH.into());
        let plan_only = args.iter().any(|a| a == "--plan");
        return match args.get(1).map(String::as_str) {
            Some("check") => pc_profile(&path, false, true),
            Some("apply") => pc_profile(&path, true, plan_only),
            _ => usage(),
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
            // `--clock`: linked (default: dsync's in-process PTP follower,
            // the service as fallback), dsync (the service only), system
            // (a test machine's own clock: CI; never on a Dante network).
            let Some(clock) = dasio::Clock::parse(flag(&args, "--clock").as_deref()) else {
                return usage();
            };
            // `--spare-blocks <n>`: block periods the DAW gets beyond the
            // first (a heavy set's late blocks), its outputs that much later.
            let spare = match flag(&args, "--spare-blocks").map(|n| n.parse::<u32>()) {
                None => 0,
                Some(Ok(n)) if n <= 1 => n,
                Some(_) => return usage(),
            };
            // `--kernel-tx`: audio TX through dasio's NDIS filter.
            let tuning = dasio::Tuning {
                spare,
                kernel_tx: args.iter().any(|a| a == "--kernel-tx"),
            };
            if cmd == "service" {
                service(instance, config.into(), ip, clock, tuning)
            } else {
                run(instance, config.into(), ip, clock, tuning)
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
    clock: dasio::Clock,
    tuning: dasio::Tuning,
) -> Result<(), String> {
    let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    dasio::run::run(instance, config, ip, source(clock), tuning, never)
}

#[cfg(windows)]
fn source(clock: dasio::Clock) -> dstack::host::ClockSource {
    match clock {
        dasio::Clock::Linked => dstack::host::ClockSource::Linked,
        dasio::Clock::Dsync => dstack::host::ClockSource::Dsync,
        dasio::Clock::System => dstack::host::ClockSource::System,
    }
}

#[cfg(windows)]
fn service(
    instance: u32,
    config: std::path::PathBuf,
    ip: std::net::Ipv4Addr,
    clock: dasio::Clock,
    tuning: dasio::Tuning,
) -> Result<(), String> {
    dasio::service::windows::dispatch(instance, config, ip, source(clock), tuning)
}

#[cfg(not(windows))]
fn service(
    _instance: u32,
    _config: std::path::PathBuf,
    _ip: std::net::Ipv4Addr,
    _clock: dasio::Clock,
    _tuning: dasio::Tuning,
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
    _clock: dasio::Clock,
    _tuning: dasio::Tuning,
) -> Result<(), String> {
    Err("Windows only".into())
}

/// Checks the PC against its profile; with `apply`, sets what differs
/// (unless `plan_only`) and checks again. Exit 1 on any problem left.
#[cfg(windows)]
fn pc_profile(path: &str, apply: bool, plan_only: bool) -> ExitCode {
    use dasio::pc_profile::{Action, check, parse, plan, takes_effect, windows::Pc};
    let p = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| parse(&t))
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dasio: the profile {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let pc = Pc::new(&p);
    if apply {
        let actions = plan(&p, &pc);
        for a in &actions {
            let what = match a {
                Action::Reg { key, name, value } => format!("registry {key}\\{name} = {value:?}"),
                Action::Ini { file, key, value } => format!("{file} {key}={value}"),
            };
            if plan_only {
                println!("would set {what} (takes effect after {})", takes_effect(a));
                continue;
            }
            match pc.apply(a) {
                Ok(()) => println!("set {what} (takes effect after {})", takes_effect(a)),
                Err(e) => println!("FAILED {what}: {e}"),
            }
        }
        if actions.is_empty() {
            println!("nothing to set");
        }
        if plan_only {
            return ExitCode::SUCCESS;
        }
    }
    let problems = check(&p, &pc);
    for pr in &problems {
        println!("PROBLEM {pr}");
    }
    let entries = p.registry.len() + p.ini.len() + p.process.len();
    if problems.is_empty() {
        println!("PC profile {}: as it should be ({entries} entries)", p.name);
        ExitCode::SUCCESS
    } else {
        println!(
            "PC profile {}: {} problems of {entries} entries",
            p.name,
            problems.len()
        );
        ExitCode::FAILURE
    }
}

#[cfg(not(windows))]
fn pc_profile(_path: &str, _apply: bool, _plan_only: bool) -> ExitCode {
    eprintln!("dasio: Windows only");
    ExitCode::FAILURE
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
