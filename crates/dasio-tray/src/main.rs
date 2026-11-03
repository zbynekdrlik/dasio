// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]

//! `dasio-tray`: the tray icon (Windows). Once a second it reads every
//! installed instance (its service `dasio-<n>`, its ASIO name, its header
//! through [`shm::peek`]) and shows [`dasio_tray::view`]'s colour, tooltip
//! and lines. One per desktop session (a named mutex). `dasio-tray --print`
//! writes the tooltip and the lines once to stdout instead (a check without
//! a desktop).

#[cfg(not(windows))]
fn main() {
    eprintln!("dasio-tray runs on Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--print") {
        let v = dasio_tray::view::Tracker::default().update(0, &win::readings());
        println!("{}", v.tooltip);
        for line in &v.lines {
            println!("{line}");
        }
        return;
    }
    if let Err(e) = win::run() {
        win::message(&format!("dasio-tray: {e}"));
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::time::Instant;

    use dasio_tray::view::{self, Colour, Reading, Service, Tracker, View};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW,
        RegGetValueW, RegOpenKeyExW,
    };
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_CONTINUE_PENDING, SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_STATUS,
        SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STOP_PENDING,
        SERVICE_STOPPED,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MB_ICONERROR, MB_OK, MSG, MessageBoxW, SetTimer,
        TranslateMessage, WM_TIMER,
    };

    /// Where the instances keep their configs and logs.
    const DATA: &str = r"C:\ProgramData\dasio";
    const ICON: u32 = 32;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn message(text: &str) {
        let t = wide(text);
        let c = wide("dasio");
        // SAFETY: valid NUL-terminated strings, no owner window.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
    }

    pub fn run() -> Result<(), String> {
        // One tray a session; a second start leaves quietly.
        let name = wide(r"Local\dasio-tray");
        // SAFETY: a valid name; the handle is held until the process ends.
        let _mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // SAFETY: plain call, right after CreateMutexW.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let version = MenuItem::new(
            format!(
                "dasio {}{}",
                env!("CARGO_PKG_VERSION"),
                option_env!("GITHUB_SHA")
                    .map_or(String::new(), |s| format!(" ({})", &s[..7.min(s.len())]))
            ),
            false,
            None,
        );
        let logs = MenuItem::new("Open logs", true, None);
        let exit = MenuItem::new("Exit (the instances keep running)", true, None);
        let mut tracker = Tracker::default();
        let start = Instant::now();
        let first = tracker.update(0, &readings());
        let (first_menu, mut lines) = menu(&version, &first, &logs, &exit)?;
        let tray: TrayIcon = TrayIconBuilder::new()
            .with_menu(Box::new(first_menu))
            .with_tooltip(&first.tooltip)
            .with_icon(icon(first.colour)?)
            .build()
            .map_err(|e| format!("the tray icon: {e}"))?;
        let mut shown = first;
        // A thread timer: WM_TIMER in this thread's queue once a second.
        // SAFETY: no window, no callback.
        unsafe { SetTimer(std::ptr::null_mut(), 0, 1000, None) };
        let menu_events = MenuEvent::receiver();
        // SAFETY: MSG is plain data.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: a valid MSG; all of this thread's messages.
            let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if got <= 0 {
                return Ok(());
            }
            if msg.message == WM_TIMER && msg.hwnd.is_null() {
                let v = tracker.update(start.elapsed().as_secs(), &readings());
                if v.lines.len() != lines.len() {
                    let (m, items) = menu(&version, &v, &logs, &exit)?;
                    lines = items;
                    tray.set_menu(Some(Box::new(m)));
                } else {
                    for (item, text) in lines.iter().zip(&v.lines) {
                        if item.text() != *text {
                            item.set_text(text);
                        }
                    }
                }
                if v.colour != shown.colour {
                    let _ = tray.set_icon(Some(icon(v.colour)?));
                }
                if v.tooltip != shown.tooltip {
                    let _ = tray.set_tooltip(Some(&v.tooltip));
                }
                shown = v;
            } else {
                // SAFETY: a message GetMessageW filled.
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(e) = menu_events.try_recv() {
                if e.id == *exit.id() {
                    return Ok(());
                }
                if e.id == *logs.id() {
                    let _ = std::process::Command::new("explorer.exe").arg(DATA).spawn();
                }
            }
        }
    }

    /// The menu: the version, a line per instance, Open logs, Exit.
    fn menu(
        version: &MenuItem,
        v: &View,
        logs: &MenuItem,
        exit: &MenuItem,
    ) -> Result<(Menu, Vec<MenuItem>), String> {
        let menu = Menu::new();
        let e = |r: Result<(), tray_icon::menu::Error>| r.map_err(|e| format!("the menu: {e}"));
        e(menu.append(version))?;
        e(menu.append(&PredefinedMenuItem::separator()))?;
        let mut items = Vec::new();
        if v.lines.is_empty() {
            e(menu.append(&MenuItem::new("No dasio instance installed", false, None)))?;
        }
        for text in &v.lines {
            let item = MenuItem::new(text, false, None);
            e(menu.append(&item))?;
            items.push(item);
        }
        e(menu.append(&PredefinedMenuItem::separator()))?;
        e(menu.append(logs))?;
        e(menu.append(exit))?;
        Ok((menu, items))
    }

    /// A filled circle in the colour, on transparent.
    fn icon(c: Colour) -> Result<Icon, String> {
        let (r, g, b) = match c {
            Colour::Green => (0x2e, 0xb8, 0x4b),
            Colour::Yellow => (0xf2, 0xb1, 0x0f),
            Colour::Red => (0xe0, 0x2f, 0x2f),
            Colour::Grey => (0x8a, 0x8a, 0x8a),
        };
        let mut px = Vec::with_capacity((ICON * ICON * 4) as usize);
        let mid = (ICON as f32 - 1.0) / 2.0;
        for y in 0..ICON {
            for x in 0..ICON {
                let d = ((x as f32 - mid).powi(2) + (y as f32 - mid).powi(2)).sqrt();
                let a = (mid + 0.5 - d).clamp(0.0, 1.0);
                px.extend_from_slice(&[r, g, b, (a * 255.0) as u8]);
            }
        }
        Icon::from_rgba(px, ICON, ICON).map_err(|e| format!("the icon: {e}"))
    }

    /// Every installed instance now.
    pub fn readings() -> Vec<Reading> {
        let entries = asio_entries();
        let mut out = Vec::new();
        for (service, n) in services() {
            let clsid = dasio_driver::ident::clsid(n)
                .map(|g| dasio_driver::ident::braced(&g))
                .unwrap_or_default();
            out.push(Reading {
                instance: n,
                name: view::name_of(n, &clsid, &entries),
                service: state(&service),
                snap: shm::peek(n).ok(),
            });
        }
        out.sort_by_key(|r| r.instance);
        out
    }

    /// An open registry key, closed on drop.
    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: a key this struct opened.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(path: &str) -> Option<Key> {
        let p = wide(path);
        let mut k: HKEY = std::ptr::null_mut();
        // SAFETY: a valid path and out pointer.
        let r = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, p.as_ptr(), 0, KEY_READ, &mut k) };
        (r == ERROR_SUCCESS).then_some(Key(k))
    }

    /// The names of `key`'s subkeys.
    fn subkeys(key: &Key) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: a valid key and buffer of `len` characters.
            let r = unsafe {
                RegEnumKeyExW(
                    key.0,
                    i,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    /// The string value `value` of `key`'s subkey `sub`.
    fn string(key: &Key, sub: &str, value: &str) -> Option<String> {
        let (s, v) = (wide(sub), wide(value));
        let mut buf = [0u16; 512];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: valid key, names and a buffer of `bytes` bytes.
        let r = unsafe {
            RegGetValueW(
                key.0,
                s.as_ptr(),
                v.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        (r == ERROR_SUCCESS).then(|| {
            let n = (bytes as usize / 2).saturating_sub(1);
            String::from_utf16_lossy(&buf[..n])
        })
    }

    /// Every ASIO entry: its name and CLSID.
    fn asio_entries() -> Vec<(String, String)> {
        let Some(k) = open(r"SOFTWARE\ASIO") else {
            return Vec::new();
        };
        subkeys(&k)
            .into_iter()
            .filter_map(|name| string(&k, &name, "CLSID").map(|c| (name, c)))
            .collect()
    }

    /// Every `dasio-<n>` service: its name and instance.
    fn services() -> Vec<(String, u32)> {
        let Some(k) = open(r"SYSTEM\CurrentControlSet\Services") else {
            return Vec::new();
        };
        subkeys(&k)
            .into_iter()
            .filter_map(|s| view::instance_of_service(&s).map(|n| (s, n)))
            .collect()
    }

    /// The service's state; a service the manager does not know: stopped.
    fn state(service: &str) -> Service {
        // SAFETY: the local manager, connect right only.
        let scm = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return Service::Other("unknown".into());
        }
        let name = wide(service);
        // SAFETY: a valid manager handle and name.
        let svc = unsafe { OpenServiceW(scm, name.as_ptr(), SERVICE_QUERY_STATUS) };
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: a valid service handle (checked) and status buffer.
        let ok = !svc.is_null() && unsafe { QueryServiceStatus(svc, &mut st) } != 0;
        // SAFETY: handles opened above, each closed once.
        unsafe {
            if !svc.is_null() {
                CloseServiceHandle(svc);
            }
            CloseServiceHandle(scm);
        }
        if !ok {
            return Service::Stopped;
        }
        match st.dwCurrentState {
            SERVICE_RUNNING => Service::Running,
            SERVICE_STOPPED => Service::Stopped,
            SERVICE_START_PENDING => Service::Other("starting".into()),
            SERVICE_STOP_PENDING => Service::Other("stopping".into()),
            SERVICE_PAUSED => Service::Other("paused".into()),
            SERVICE_PAUSE_PENDING => Service::Other("pausing".into()),
            SERVICE_CONTINUE_PENDING => Service::Other("resuming".into()),
            other => Service::Other(format!("state {other}")),
        }
    }
}
