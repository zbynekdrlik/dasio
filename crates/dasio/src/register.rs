//! The driver's registration for one instance (plan Task 7): its COM class
//! (`HKLM\SOFTWARE\Classes\CLSID\{guid}`, the DLL as its in-process server)
//! and its ASIO entry (`HKLM\SOFTWARE\ASIO\<name>`), which DAWs list.

use dasio_driver::ident::{braced, clsid, driver_name};

/// One registry value under `HKLM`: key, value name (empty: the default),
/// string data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub data: String,
}

fn e(key: &str, name: &str, data: &str) -> Entry {
    Entry {
        key: key.to_string(),
        name: name.to_string(),
        data: data.to_string(),
    }
}

/// Instance `n`'s values, the DLL at `dll`, listed to DAWs as `name` (the
/// driver's default name when `None`); `None` outside the instances.
pub fn entries(n: u32, dll: &str, name: Option<&str>) -> Option<Vec<Entry>> {
    let guid = braced(&clsid(n)?);
    let name = name.map_or_else(|| driver_name(n), str::to_string);
    let class = format!("SOFTWARE\\Classes\\CLSID\\{guid}");
    let server = format!("{class}\\InprocServer32");
    let asio = format!("SOFTWARE\\ASIO\\{name}");
    Some(vec![
        e(&class, "", &name),
        e(&server, "", dll),
        e(&server, "ThreadingModel", "Apartment"),
        e(&asio, "CLSID", &guid),
        e(&asio, "Description", &name),
    ])
}

/// The keys `unregister` removes for instance `n` listed as `name` (the
/// default name when `None`), whole trees.
pub fn keys(n: u32, name: Option<&str>) -> Option<[String; 2]> {
    let guid = braced(&clsid(n)?);
    let name = name.map_or_else(|| driver_name(n), str::to_string);
    Some([
        format!("SOFTWARE\\Classes\\CLSID\\{guid}"),
        format!("SOFTWARE\\ASIO\\{name}"),
    ])
}

#[cfg(windows)]
pub use win::{apply, remove};

#[cfg(windows)]
mod win {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };

    use super::Entry;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Writes `entries` under HKLM (administrator).
    pub fn apply(entries: &[Entry]) -> io::Result<()> {
        for en in entries {
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: valid NUL-terminated strings and out pointers.
            let r = unsafe {
                RegCreateKeyExW(
                    HKEY_LOCAL_MACHINE,
                    wide(&en.key).as_ptr(),
                    0,
                    std::ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    std::ptr::null(),
                    &mut key,
                    std::ptr::null_mut(),
                )
            };
            if r != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(r as i32));
            }
            let data = wide(&en.data);
            let name = wide(&en.name);
            // SAFETY: an open key; a REG_SZ of `data`'s bytes with its NUL.
            let r = unsafe {
                RegSetValueExW(
                    key,
                    if en.name.is_empty() {
                        std::ptr::null()
                    } else {
                        name.as_ptr()
                    },
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            // SAFETY: the key opened above.
            unsafe { RegCloseKey(key) };
            if r != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(r as i32));
            }
        }
        Ok(())
    }

    /// Removes the trees `keys` under HKLM; one already gone is fine.
    pub fn remove(keys: &[String]) -> io::Result<()> {
        for k in keys {
            // SAFETY: a valid NUL-terminated subkey.
            let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(k).as_ptr()) };
            if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
                return Err(io::Error::from_raw_os_error(r as i32));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_for_instance_2() {
        let g = "{5D4A7C21-9B3E-4F6A-8C11-2E475A90D102}";
        let class = format!("SOFTWARE\\Classes\\CLSID\\{g}");
        let server = format!("{class}\\InprocServer32");
        assert_eq!(
            entries(2, "C:\\dasio\\dasio.dll", None).unwrap(),
            vec![
                e(&class, "", "dasio 2"),
                e(&server, "", "C:\\dasio\\dasio.dll"),
                e(&server, "ThreadingModel", "Apartment"),
                e("SOFTWARE\\ASIO\\dasio 2", "CLSID", g),
                e("SOFTWARE\\ASIO\\dasio 2", "Description", "dasio 2"),
            ]
        );
        assert_eq!(entries(0, "x", None), None);
        assert_eq!(keys(2, None).unwrap()[1], "SOFTWARE\\ASIO\\dasio 2");
        let named = entries(1, "x", Some("master-asio-dante")).unwrap();
        assert_eq!(named[0].data, "master-asio-dante");
        assert_eq!(named[3].key, "SOFTWARE\\ASIO\\master-asio-dante");
        assert_eq!(
            keys(1, Some("master-asio-dante")).unwrap()[1],
            "SOFTWARE\\ASIO\\master-asio-dante"
        );
    }
}
