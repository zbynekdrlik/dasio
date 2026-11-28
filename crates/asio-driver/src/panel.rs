//! The DAW's "Hardware Setup" button (`controlPanel()`, dasio#248): it opens
//! dtray's settings window. dtray.exe sits beside the driver's DLL (the
//! installer puts both in one folder), so the path comes from the DLL's own
//! path, never a fixed install dir. This module decides: where dtray is,
//! whether it can be started, which `ASIOError` the DAW gets. The module
//! path and the process start are Windows glue in `driver.rs`. Portable, so
//! it is tested on every OS.

use std::io;
use std::path::{Path, PathBuf};

use crate::abi::{ASE_NOT_PRESENT, ASE_OK, ASIOError};

/// The settings program, installed beside the driver DLL.
pub const DTRAY_EXE: &str = "dtray.exe";

/// What starts dtray's per-instance settings window (it takes no instance:
/// the window lists them, dstack#246).
pub const DTRAY_ARGS: [&str; 1] = ["--settings"];

/// The dtray.exe beside `dll`; `None` for a path with no folder part.
pub fn dtray_beside(dll: &Path) -> Option<PathBuf> {
    let dir = dll.parent().filter(|d| !d.as_os_str().is_empty())?;
    Some(dir.join(DTRAY_EXE))
}

/// How `controlPanel()` ended.
#[derive(Debug)]
pub enum Panel {
    /// dtray was started (and is not waited for).
    Opened(PathBuf),
    /// The driver's own path is unknown.
    NoModule,
    /// No dtray.exe beside the driver.
    Missing(PathBuf),
    /// dtray.exe is there but did not start.
    SpawnFailed(PathBuf, io::Error),
}

impl Panel {
    /// What the DAW gets: `ASE_OK` once dtray started, else `ASE_NOT_PRESENT`.
    pub fn ase(&self) -> ASIOError {
        match self {
            Panel::Opened(_) => ASE_OK,
            Panel::NoModule | Panel::Missing(_) | Panel::SpawnFailed(..) => ASE_NOT_PRESENT,
        }
    }

    /// One line for the debugger's output (nothing else in the DAW's
    /// process shows it).
    pub fn describe(&self) -> String {
        match self {
            Panel::Opened(p) => format!("dasio controlPanel: started {}", p.display()),
            Panel::NoModule => "dasio controlPanel: the driver's own path is unknown".into(),
            Panel::Missing(p) => format!("dasio controlPanel: {} is missing", p.display()),
            Panel::SpawnFailed(p, e) => {
                format!("dasio controlPanel: {} did not start: {e}", p.display())
            }
        }
    }
}

/// Opens dtray's settings window for the driver whose DLL is `dll`. `exists`
/// says whether a file is there, `spawn` starts the program with the
/// arguments and returns at once (it never waits for the child).
pub fn open(
    dll: Option<&Path>,
    exists: impl FnOnce(&Path) -> bool,
    spawn: impl FnOnce(&Path, &[&str]) -> io::Result<()>,
) -> Panel {
    let Some(dtray) = dll.and_then(dtray_beside) else {
        return Panel::NoModule;
    };
    if !exists(&dtray) {
        return Panel::Missing(dtray);
    }
    match spawn(&dtray, &DTRAY_ARGS) {
        Ok(()) => Panel::Opened(dtray),
        Err(e) => Panel::SpawnFailed(dtray, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn dll() -> PathBuf {
        PathBuf::from("/opt/dasio/dasio_driver.dll")
    }

    /// The dtray path the driver above gets (the separator is the OS's).
    fn dtray() -> PathBuf {
        PathBuf::from("/opt/dasio").join("dtray.exe")
    }

    #[test]
    fn dtray_is_in_the_dlls_folder() {
        assert_eq!(dtray_beside(&dll()), Some(dtray()));
        assert_eq!(
            dtray_beside(Path::new("dasio_driver.dll")),
            None,
            "no folder part"
        );
        assert_eq!(dtray_beside(Path::new("")), None);
    }

    #[test]
    fn a_present_dtray_is_started_with_settings_and_the_daw_gets_ok() {
        let seen = RefCell::new(None);
        let p = open(
            Some(&dll()),
            |f| f == dtray(),
            |path, args| {
                *seen.borrow_mut() = Some((
                    path.to_path_buf(),
                    args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                ));
                Ok(())
            },
        );
        assert!(matches!(&p, Panel::Opened(f) if *f == dtray()));
        assert_eq!(p.ase(), ASE_OK);
        assert_eq!(
            seen.into_inner(),
            Some((dtray(), vec!["--settings".to_string()]))
        );
        assert_eq!(
            p.describe(),
            format!("dasio controlPanel: started {}", dtray().display())
        );
    }

    #[test]
    fn a_missing_dtray_is_not_started_and_is_not_present() {
        let started = RefCell::new(false);
        let p = open(
            Some(&dll()),
            |_| false,
            |_, _| {
                *started.borrow_mut() = true;
                Ok(())
            },
        );
        assert!(matches!(&p, Panel::Missing(f) if *f == dtray()));
        assert_eq!(p.ase(), ASE_NOT_PRESENT);
        assert!(!started.into_inner());
        assert_eq!(
            p.describe(),
            format!("dasio controlPanel: {} is missing", dtray().display())
        );
    }

    #[test]
    fn a_failed_start_is_not_present_with_the_reason() {
        let p = open(
            Some(&dll()),
            |_| true,
            |_, _| Err(io::Error::other("access denied")),
        );
        assert!(matches!(&p, Panel::SpawnFailed(..)));
        assert_eq!(p.ase(), ASE_NOT_PRESENT);
        assert_eq!(
            p.describe(),
            format!(
                "dasio controlPanel: {} did not start: access denied",
                dtray().display()
            )
        );
    }

    #[test]
    fn an_unknown_driver_path_starts_nothing() {
        for unknown in [None, Some(Path::new("dasio_driver.dll"))] {
            let p = open(
                unknown,
                |_| panic!("looked for a file"),
                |_, _| panic!("started a program"),
            );
            assert!(matches!(p, Panel::NoModule));
            assert_eq!(p.ase(), ASE_NOT_PRESENT);
            assert_eq!(
                p.describe(),
                "dasio controlPanel: the driver's own path is unknown"
            );
        }
    }
}
