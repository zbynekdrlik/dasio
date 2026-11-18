// SPDX-License-Identifier: GPL-3.0-or-later

//! `dasio run --instance <n> --config <device.toml> [--ip <address>|--iface <name>]`
//! (the config's `iface = "<name>"` wins over both, dstack#246):
//! one instance's Dante device and its shared memory for the driver;
//! `dasio service ...` (same flags): the same, as the SCM starts it.
//! `dasio register --instance <n> --dll <path> [--name <shown to DAWs>]` and
//! `dasio unregister --instance <n>`: the driver's registry entries
//! (administrator).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: dasio run --instance <n> --config <device.toml> [--ip <address> | --iface <interface name>] [--log <file>] [--clock linked|dsync|system] [--spare-blocks <n>]\n       \
         dasio service --instance <n> --config <device.toml> [--ip <address> | --iface <interface name>] --log <file> [--clock linked|dsync|system] [--spare-blocks <n>]\n       \
         dasio register --instance <n> --dll <path> [--name <shown to DAWs>]\n       \
         dasio stop --instance <n>\n       \
         dasio probe --instance <n> [--seconds <s>] [--affinity <hex mask>] [--rate <Hz>]\n       \
         dasio jitter --cpus <n,n,..> --seconds <s>\n       \
         dasio pc-profile check|apply|layout|keep [--profile <file>] [--plan]\n       \
         dasio pc-profile nic-driver [--profile <file>] --hardware-id <id>... --file <name>=<sha256>... --signature <status> --signer <subject>\n       \
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
        // `dasio pc-profile check|apply|layout [--profile <file>] [--plan]`
        // (dasio#5): the PC's Windows settings against its profile; exit
        // 1 on a problem, 2 on a must-have one (dasio#128).
        let path =
            flag(&args, "--profile").unwrap_or_else(|| dasio::pc_profile::DEFAULT_PATH.into());
        let plan_only = args.iter().any(|a| a == "--plan");
        return match args.get(1).map(String::as_str) {
            Some("check") => pc_profile(&path, false, true),
            Some("apply") => pc_profile(&path, true, plan_only),
            Some("layout") => pc_layout(&path),
            Some("keep") => pc_keep(&path),
            Some("nic-driver") => pc_nic_driver(&path, &args[2..]),
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
            // `--rate <Hz>`: the host's rate, as a DAW's project sets it
            // (dasio#107: the device converts); 96 kHz unless given.
            let rate = match flag(&args, "--rate").map(|r| r.parse::<u32>()) {
                None => 96_000,
                Some(Ok(r)) => r,
                Some(Err(_)) => return usage(),
            };
            probe(instance, seconds, affinity, rate)
        }
        "unregister" => match listed().map(|on_pc| {
            dasio::register::unregister_keys(instance, flag(&args, "--name").as_deref(), &on_pc)
        }) {
            Ok(Some(keys)) => remove(&keys),
            Ok(None) => return usage(),
            Err(e) => Err(e),
        },
        "run" | "service" => {
            quiet_crashes();
            // A panic in any thread reaches the instance log (dasio#28 E17).
            dasio::health::log_panics();
            let Some(config) = flag(&args, "--config") else {
                return usage();
            };
            // The device's place: `iface = "<name>"` in the config (dtray's
            // settings page, dstack#246), else `--ip <address>` or
            // `--iface <interface name>` (dasio#29): exactly one, once.
            let text = match std::fs::read_to_string(&config) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("dasio: {config}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let place = match dasio::place::resolve(&args, &text) {
                Ok((place, from)) => {
                    eprintln!("dasio: the device's place {place:?} ({from:?})");
                    place
                }
                Err(e) => {
                    eprintln!("dasio: {config}: {e}");
                    return usage();
                }
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
            // Audio TX through dasio's NDIS filter on every real clock: a
            // device without it does not start (dasio#125). An old service
            // path's `--kernel-tx` is ignored.
            let tuning = dasio::Tuning {
                spare,
                kernel_tx: dasio::kernel_tx_required(clock),
            };
            if cmd == "service" {
                // The service looks the address up once the box holds it.
                service(instance, config.into(), place, clock, tuning)
            } else {
                // Run by hand: the interface's address as it is now.
                dasio::place::address(&place, true)
                    .and_then(|ip| run(instance, config.into(), ip, clock, tuning))
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

/// A crash of the device process ends it at once (no error box, no WER
/// UI): the SCM restarts it instead of it holding its Dante ports.
#[cfg(windows)]
fn quiet_crashes() {
    let mode = dasio::run::quiet_crashes();
    tracing::info!(
        error_mode = mode,
        "crashes end the process without a dialog"
    );
}

#[cfg(not(windows))]
fn quiet_crashes() {}

#[cfg(windows)]
fn apply(entries: &[dasio::register::Entry]) -> Result<(), String> {
    dasio::register::apply(entries).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn remove(keys: &[String]) -> Result<(), String> {
    for k in keys {
        println!("removing HKLM\\{k}");
    }
    dasio::register::remove(keys).map_err(|e| e.to_string())
}

/// The ASIO entries on this PC, for `unregister` (#112).
#[cfg(windows)]
fn listed() -> Result<Vec<(String, String)>, String> {
    dasio::register::listed().map_err(|e| e.to_string())
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
fn probe(instance: u32, seconds: u64, affinity: Option<usize>, rate: u32) -> Result<(), String> {
    if let Some(mask) = affinity {
        dasio::probe::set_affinity(mask)?;
    }
    dasio::probe::high_priority()?;
    let r = dasio::probe::probe_at(instance, seconds, 0, rate)?;
    let min = r.round_trips.iter().min();
    let max = r.round_trips.iter().max();
    println!(
        "probe instance {instance}: rate {} block {} latency_in {} latency_out {} switches {} emitted {} arrived {} round_trip_min {:?} round_trip_max {:?} late {} missed {} samples {} max_gap_us {} gaps_over_1ms {} gap_blocks {} max_switch_us {} vm_stalls {} vm_stall_max_us {} vm_gap_blocks {} pump_lost_late_blocks {} pump_lost_device_blocks {}",
        r.rate,
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
        r.max_switch_us,
        r.vm_stalls,
        r.vm_stall_max_us,
        r.vm_gap_blocks,
        r.pump_lost_late_blocks,
        r.pump_lost_device_blocks
    );
    for (second, (peak, at)) in r.rx_peaks.iter().enumerate() {
        println!("rx1 second {second}: peak {peak} at sample {at}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn probe(
    _instance: u32,
    _seconds: u64,
    _affinity: Option<usize>,
    _rate: u32,
) -> Result<(), String> {
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
    place: dstack::daemon::Place,
    clock: dasio::Clock,
    tuning: dasio::Tuning,
) -> Result<(), String> {
    dasio::service::windows::dispatch(instance, config, place, source(clock), tuning)
}

#[cfg(not(windows))]
fn service(
    _instance: u32,
    _config: std::path::PathBuf,
    _place: dstack::daemon::Place,
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
fn listed() -> Result<Vec<(String, String)>, String> {
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
    use dasio::pc_profile::{
        check, describe, load, must_have_failure, plan, takes_effect,
        windows::{Pc, cpu_count},
    };
    // The built-in baseline (dasio#128) with this PC's own profile. A
    // profile that does not load leaves the must-have items unchecked:
    // exit 2, so install.ps1 refuses (dasio#128).
    let p = match cpu_count().and_then(|cpus| load(std::path::Path::new(path), cpus)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dasio: the profile {path}: {e}");
            println!(
                "MUST-HAVE the PC profile does not load, so dasio's must-have items cannot be checked: {e}"
            );
            return ExitCode::from(2);
        }
    };
    let pc = Pc::new(&p);
    if apply {
        let actions = plan(&p, &pc);
        for a in &actions {
            // The real step: a power-plan value goes through powercfg
            // (dasio#128).
            let what = describe(a);
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
    for d in &p.disk {
        match dasio::pc_profile::Reader::free_bytes(&pc, &d.drive) {
            Ok(free) => println!(
                "disk {} free: {} (floor {} GB)",
                d.drive,
                dasio::pc_profile::gb_down(free),
                d.min_free_gb
            ),
            Err(e) => println!("disk {} free: unreadable ({e})", d.drive),
        }
    }
    let entries = p.entries();
    if problems.is_empty() {
        println!("PC profile {}: as it should be ({entries} entries)", p.name);
        ExitCode::SUCCESS
    } else {
        println!(
            "PC profile {}: {} problems of {entries} entries",
            p.name,
            problems.len()
        );
        let texts: Vec<String> = problems.iter().map(ToString::to_string).collect();
        match must_have_failure(&texts) {
            // install.ps1 refuses on 2 (dasio#128).
            Some(why) => {
                println!("MUST-HAVE {why}");
                ExitCode::from(2)
            }
            None => ExitCode::FAILURE,
        }
    }
}

#[cfg(not(windows))]
fn pc_profile(_path: &str, _apply: bool, _plan_only: bool) -> ExitCode {
    eprintln!("dasio: Windows only");
    ExitCode::FAILURE
}

/// `dasio pc-profile nic-driver --hardware-id <id>... --file
/// <name>=<sha256>... --signature <status> --signer <subject>` (dasio#128,
/// install.ps1's `-NicDriver`): whether the package install.ps1 read is the
/// one the baseline (with the PC's own profile) pins for the Dante NIC's
/// family; exit 0 with `INF`/`VERSION` lines, 2 refused.
fn pc_nic_driver(path: &str, args: &[String]) -> ExitCode {
    let p = match dasio::pc_profile::load_merged(std::path::Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            println!("REFUSED the PC profile does not load: {e}");
            return ExitCode::from(2);
        }
    };
    // `--profile <file>` is main's, not the decision's.
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--profile" {
            it.next();
        } else {
            rest.push(a.clone());
        }
    }
    let (lines, code) = dasio::pc_profile::nic_driver::command(&p.nic_driver, &rest);
    for l in &lines {
        println!("{l}");
    }
    ExitCode::from(code)
}

/// The CPU layout this PC's profile is held to (dasio#128), for its
/// Process Lasso rules. From the site profile's `instances` alone, so it
/// also shows a layout whose placeholders do not fit (the PC's check names
/// those).
#[cfg(windows)]
fn pc_layout(path: &str) -> ExitCode {
    use dasio::pc_profile::{Layout, site_instances, windows::cpu_count};
    let instances = site_instances(std::path::Path::new(path));
    match cpu_count().and_then(|cpus| instances.map(|n| (cpus, n))) {
        Ok((cpus, instances)) => {
            let l = Layout::of(cpus, instances);
            println!(
                "{cpus} CPUs, {instances} dasio instance(s): {}",
                l.describe()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dasio: the profile {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn pc_layout(_path: &str) -> ExitCode {
    eprintln!("dasio: Windows only");
    ExitCode::FAILURE
}

/// `dasio pc-profile keep` (dasio#185, the `dasio-pc-keep` SYSTEM task
/// install.ps1 registers): every running process of each `[[process]]`
/// entry gets the entry's affinity and priority class, every
/// [`keep::EVERY_MS`](dasio::pc_profile::keep::EVERY_MS), what Process
/// Lasso did. Runs until stopped, on the system CPUs below normal; reloads
/// the profile when its file changes; logs each change to
/// [`keep::LOG_PATH`](dasio::pc_profile::keep::LOG_PATH).
#[cfg(windows)]
fn pc_keep(path: &str) -> ExitCode {
    use dasio::pc_profile::{
        Layout, keep, load,
        windows::{cpu_count, keep::pass},
    };
    use std::io::Write;
    let log = |line: &str| {
        let p = std::path::Path::new(keep::LOG_PATH);
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if size > keep::LOG_MAX_BYTES && std::fs::rename(p, p.with_extension("log.1")).is_err() {
            // A reader holds it without delete sharing: start it over past
            // twice the cap rather than let it grow.
            if size > 2 * keep::LOG_MAX_BYTES {
                let _ = std::fs::File::create(p);
            }
        }
        let stamp = keep::utc_stamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{stamp} {line}");
            }
            Err(e) => eprintln!("dasio keep: {}: {e}", keep::LOG_PATH),
        }
    };
    let cpus = match cpu_count() {
        Ok(n) => n,
        Err(e) => {
            log(&format!("the keeper did not start: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // The system CPUs do not depend on the instance count: placed before
    // the profile loads, so a profile that never loads leaves it placed.
    log(&place_keeper(Layout::of(cpus, 1).system_mask()));
    let file = std::path::Path::new(path);
    let mut profile: Option<dasio::pc_profile::Profile> = None;
    let mut stamp = None;
    let mut tried = false;
    let mut last_error = None;
    let mut fights = keep::Fights::default();
    let mut failed = std::collections::HashSet::new();
    let start = std::time::Instant::now();
    loop {
        let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok();
        // A load only at start and when the file changes: a profile that
        // does not load is not parsed again every pass.
        if !tried || modified != stamp {
            tried = true;
            stamp = modified;
            match load(file, cpus) {
                Ok(p) => {
                    let held = keep::held(&p);
                    log(&format!(
                        "the keeper holds {} process entries: {}",
                        held.len(),
                        held.iter()
                            .map(|e| e.exe.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    profile = Some(p);
                    last_error = None;
                    // The keeper's own changes after an edit are no fight,
                    // and a failure may be gone with the new entries.
                    fights = keep::Fights::default();
                    failed.clear();
                }
                Err(e) => {
                    if last_error.as_ref() != Some(&e) {
                        let held = if profile.is_some() {
                            "it keeps the last one that loaded"
                        } else {
                            "it holds nothing until one loads"
                        };
                        log(&format!("the PC profile does not load, {held}: {e}"));
                    }
                    last_error = Some(e);
                }
            }
        }
        if let Some(p) = &profile {
            let now = start.elapsed().as_millis() as u64;
            for line in pass(p, &mut fights, &mut failed, now) {
                log(&line);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(keep::EVERY_MS));
    }
}

/// The keeper's own process: on the system CPUs (`mask`) below normal, off
/// dasio's and the DAW's CPUs. The line says what it got.
#[cfg(windows)]
fn place_keeper(mask: u64) -> String {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass, SetProcessAffinityMask,
    };
    // SAFETY: the pseudo-handle of this process.
    let prio = unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) } != 0;
    // SAFETY: as above; a mask of this PC's CPUs from its layout.
    let aff = unsafe { SetProcessAffinityMask(GetCurrentProcess(), mask as usize) } != 0;
    format!("the keeper runs below normal ({prio}) on CPUs 0x{mask:X} ({aff})")
}

#[cfg(not(windows))]
fn pc_keep(_path: &str) -> ExitCode {
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
