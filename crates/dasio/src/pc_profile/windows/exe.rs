//! Removing an `[[absent]]` exe (dasio#128): the registry writes before its
//! uninstaller, the uninstaller (through the Windows caller when the entry
//! names it), the look, the fallback. The order is
//! [`absent::exe_steps`]'s (portable, tested); this is the glue.

use std::path::Path;
use std::process::Command;

use super::super::Value;
use super::super::absent::{self, ExeStep, Fallback, PreWrite};
use super::{file_there, folder_names, run_fallback, system_root, system32_tool, write};

/// How long `apply` waits for an uninstaller: a hung or interactive one
/// must not hold the boot-time apply or install.ps1 for good (dasio#128).
const UNINSTALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Removes the program `exe` (an [`absent::Removal::Exe`]). Its absence
/// already is fine.
pub(super) fn remove(
    exe: &str,
    uninstall: &[String],
    before: &[&'static PreWrite],
    windows_caller: bool,
    fallback: Option<&Fallback>,
) -> Result<(), String> {
    if !file_there(exe)? {
        return Ok(());
    }
    let Some((first, args)) = uninstall.split_first() else {
        return Err(format!("{exe}: no uninstaller given"));
    };
    let prog = absent::resolve_uninstaller(first, folder_names, |p| Path::new(p).is_file())
        .ok_or_else(|| format!("{exe}: its uninstaller {first} was not found"))?;
    let mut ran = String::new();
    // The caller's copy, while it exists; an error after it waits for the
    // copy's delete.
    let mut caller: Option<String> = None;
    let mut pending: Result<(), String> = Ok(());
    for step in absent::exe_steps(args, before, windows_caller) {
        match step {
            ExeStep::Write(w) => {
                // `plan` takes it from `absent::PRE_UNINSTALL_WRITES` (key,
                // name and data in code): no profile text reaches the write.
                write(w.key, w.name, &Value::String(w.data.to_string()))
                    .map_err(|e| format!("{exe}: before the uninstaller: {e}"))?;
                println!(
                    "before the uninstaller of {exe}: set {} = {:?} (REG_SZ)",
                    w.path(),
                    w.data
                );
            }
            ExeStep::CopyCaller => {
                let path = copy_caller().map_err(|e| format!("{exe}: {e}"))?;
                println!(
                    "the uninstaller of {exe} runs through {path} (a copy of {})",
                    absent::CALLER_SOURCE
                );
                caller = Some(path);
            }
            ExeStep::Uninstall(args) => {
                let what = format!("{prog} {}", args.join(" "));
                match command(&prog, &args, caller.as_deref())
                    .and_then(|mut cmd| run_bounded(&mut cmd, &what))
                {
                    Ok(status) => ran = format!("ran {what} ({status})"),
                    Err(e) => pending = Err(e),
                }
            }
            ExeStep::DeleteCaller => {
                if let Some(path) = caller.take() {
                    match std::fs::remove_file(&path) {
                        Ok(()) => println!("deleted {path}"),
                        Err(e) => {
                            let e = format!("deleting {path}: {e}");
                            pending = Err(match pending {
                                Err(u) => format!("{u}; {e}"),
                                Ok(()) => e,
                            });
                        }
                    }
                }
            }
            ExeStep::Look => {
                std::mem::replace(&mut pending, Ok(()))?;
                // An uninstaller's exit code says little (some end non-zero
                // after a good uninstall): the exe gone is what counts.
                if !file_there(exe)? {
                    return Ok(());
                }
            }
        }
    }
    let left = format!("{ran}: {exe} is still there");
    let Some(fb) = fallback else {
        return Err(left);
    };
    // EdgeUpdate's /uninstall keeps itself while WebView2 is installed
    // (dasio#128, dsg + mbc 16.11.2026).
    println!("fallback for {exe}: {left}");
    run_fallback(fb).map_err(|e| format!("{left}; fallback: {e}"))?;
    if file_there(exe)? {
        return Err(format!("{left}; after the fallback too"));
    }
    Ok(())
}

/// The uninstaller's process: `prog args` itself, or through the caller's
/// copy with [`absent::caller_command`]'s line.
fn command(prog: &str, args: &[String], caller: Option<&str>) -> Result<Command, String> {
    let Some(path) = caller else {
        let mut c = Command::new(prog);
        c.args(args);
        return Ok(c);
    };
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(path);
    c.raw_arg(absent::caller_command(prog, args)?);
    Ok(c)
}

/// Copies [`absent::CALLER_SOURCE`] to [`absent::caller_path`] and gives
/// the copy's path. A file already there is replaced only when it is
/// byte for byte that source (a copy an interrupted apply left); any other
/// is refused, never overwritten.
fn copy_caller() -> Result<String, String> {
    let root = system_root()?;
    let path = absent::caller_path(&root);
    let source = format!(r"{root}\{}", absent::CALLER_SOURCE);
    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
    };
    let mut to = match open() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let same = std::fs::read(&path).ok() == std::fs::read(&source).ok()
                && Path::new(&source).is_file();
            if !same {
                return Err(format!(
                    "the Windows caller {path}: a file not copied from {source} is there; dasio leaves it"
                ));
            }
            std::fs::remove_file(&path)
                .map_err(|e| format!("the Windows caller {path}: a left copy: {e}"))?;
            println!("deleted a left copy of the Windows caller {path}");
            open()
        }
        other => other,
    }
    .map_err(|e| format!("the Windows caller {path}: {e}"))?;
    let copied =
        std::fs::File::open(&source).and_then(|mut from| std::io::copy(&mut from, &mut to));
    drop(to);
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&path);
        return Err(format!("copying {source} to {path}: {e}"));
    }
    Ok(path)
}

/// Runs `cmd` (`what` names it) and waits at most [`UNINSTALL_LIMIT`];
/// past it the process and its children (the caller's uninstaller) are
/// ended (`taskkill /T /F`) and the error says so.
fn run_bounded(cmd: &mut Command, what: &str) -> Result<std::process::ExitStatus, String> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{what}: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("{what}: {e}"))? {
            return Ok(status);
        }
        if start.elapsed() > UNINSTALL_LIMIT {
            let pid = child.id().to_string();
            let tree = system32_tool("taskkill", &["/T", "/F", "/PID", pid.as_str()]);
            let _ = child.kill();
            let _ = child.wait();
            let tree = tree.err().map_or(String::new(), |e| format!(" ({e})"));
            return Err(format!(
                "{what}: still running after {} s, stopped{tree}",
                UNINSTALL_LIMIT.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
