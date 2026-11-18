//! Reading and disabling one device through the configuration manager
//! (cfgmgr32, dasio#181): the Windows glue of
//! [`device`](super::super::device), and the device tree its guard reads
//! (the present network adapters, a device's parents, what stops with it).
//! No process is started, so the bridge's in-process check reads devices
//! too.

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DISABLE_PERSIST, CM_DISABLE_UI_NOT_OK, CM_DRP_CONFIGFLAGS, CM_Disable_DevNode,
    CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW,
    CM_Get_DevNode_Registry_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_ID_List_SizeW,
    CM_Get_Device_ID_ListW, CM_Get_Device_IDW, CM_Get_Parent, CM_LOCATE_DEVNODE_NORMAL,
    CM_LOCATE_DEVNODE_PHANTOM, CM_Locate_DevNodeW, CM_Set_DevNode_Registry_PropertyW,
    CONFIGFLAG_DISABLED, CONFIGRET, CR_BUFFER_SMALL, CR_NO_SUCH_DEVNODE, CR_NO_SUCH_VALUE,
    CR_REMOVE_VETOED, CR_SUCCESS, DN_HAS_PROBLEM, MAX_DEVICE_ID_LEN,
};
use windows_sys::Win32::Devices::Properties::{
    DEVPKEY_Device_Children, DEVPKEY_Device_RemovalRelations, DEVPROP_TYPE_STRING_LIST,
};
use windows_sys::Win32::Foundation::DEVPROPKEY;

use super::super::device::{Adapter, DevState, Devices, PROBLEM_DISABLED};
use super::super::multi_sz_from_wide;
use super::{Pc, from_wide, wide};

/// The most parents walked from a device (the root's included): far more
/// than any device tree has between a device and its root.
const MAX_DEPTH: usize = 64;

/// The network adapters' setup class (GUID_DEVCLASS_NET).
const NET_CLASS_GUID: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";

/// How often a list is read again when it grew between its size and its
/// read (a device arriving then).
const LIST_TRIES: usize = 4;

/// A CONFIGRET as an error text.
fn cr_text(call: &str, cr: CONFIGRET) -> String {
    if cr == CR_REMOVE_VETOED {
        format!(
            "{call}: the device's driver or an application refused to let it stop (CONFIGRET {cr}, CR_REMOVE_VETOED)"
        )
    } else {
        format!("{call}: CONFIGRET {cr}")
    }
}

/// The devnode of `id`: `(handle, present)`; `None` when the PC has no
/// such device, connected or not.
fn locate(id: &str) -> Result<Option<(u32, bool)>, String> {
    let w = wide(id);
    for (flags, present) in [
        (CM_LOCATE_DEVNODE_NORMAL, true),
        (CM_LOCATE_DEVNODE_PHANTOM, false),
    ] {
        let mut dev = 0u32;
        // SAFETY: a NUL-terminated instance path and an out handle.
        let cr = unsafe { CM_Locate_DevNodeW(&mut dev, w.as_ptr(), flags) };
        match cr {
            CR_SUCCESS => return Ok(Some((dev, present))),
            CR_NO_SUCH_DEVNODE => {}
            cr => return Err(cr_text("CM_Locate_DevNode", cr)),
        }
    }
    Ok(None)
}

/// The devnode's ConfigFlags; 0 when it has none.
fn config_flags(dev: u32) -> Result<u32, String> {
    let mut flags = 0u32;
    let mut len = std::mem::size_of::<u32>() as u32;
    let mut kind = 0u32;
    // SAFETY: a 4-byte buffer for a REG_DWORD property, its size in `len`.
    let cr = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            dev,
            CM_DRP_CONFIGFLAGS,
            &mut kind,
            (&mut flags as *mut u32).cast(),
            &mut len,
            0,
        )
    };
    match cr {
        CR_SUCCESS => Ok(flags),
        CR_NO_SUCH_VALUE => Ok(0),
        cr => Err(cr_text("reading ConfigFlags", cr)),
    }
}

/// The state of `dev`, `present` as [`locate`] found it.
fn read(dev: u32, present: bool) -> Result<DevState, String> {
    let config_disabled = config_flags(dev)? & CONFIGFLAG_DISABLED != 0;
    if !present {
        return Ok(DevState {
            present,
            problem: None,
            config_disabled,
        });
    }
    let (mut status, mut problem) = (0u32, 0u32);
    // SAFETY: two out u32s and a devnode handle from CM_Locate_DevNode.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, dev, 0) };
    if cr != CR_SUCCESS {
        return Err(cr_text("CM_Get_DevNode_Status", cr));
    }
    Ok(DevState {
        present,
        problem: (status & DN_HAS_PROBLEM != 0).then_some(problem),
        config_disabled,
    })
}

/// The instance path of devnode `dev`.
fn device_id(dev: u32) -> Result<String, String> {
    let mut buf = vec![0u16; MAX_DEVICE_ID_LEN as usize + 1];
    // SAFETY: a buffer of `buf.len()` UTF-16 units; the id is at most
    // MAX_DEVICE_ID_LEN plus its NUL.
    let cr = unsafe { CM_Get_Device_IDW(dev, buf.as_mut_ptr(), buf.len() as u32, 0) };
    if cr != CR_SUCCESS {
        return Err(cr_text("CM_Get_Device_ID", cr));
    }
    Ok(from_wide(&buf))
}

/// Instance path `id`'s parents, nearest first, up to the root.
fn walk_up(id: &str) -> Result<Vec<String>, String> {
    let (mut dev, _) = locate(id)?.ok_or("not found")?;
    let mut out = Vec::new();
    for _ in 0..=MAX_DEPTH {
        let mut parent = 0u32;
        // SAFETY: an out handle and a devnode handle from CM_Locate_DevNode
        // or an earlier CM_Get_Parent.
        match unsafe { CM_Get_Parent(&mut parent, dev, 0) } {
            CR_SUCCESS => {}
            CR_NO_SUCH_DEVNODE => return Ok(out),
            cr => return Err(cr_text("CM_Get_Parent", cr)),
        }
        out.push(device_id(parent)?);
        dev = parent;
    }
    Err(format!("more than {MAX_DEPTH} parents"))
}

/// The Dante NIC's instance path `id`, then each of its parents' up to the
/// root; empty when the NIC or a parent was not read (then `[[device]]`
/// disables nothing).
pub fn chain(id: &str) -> Vec<String> {
    match walk_up(id) {
        Ok(parents) => std::iter::once(id.to_string()).chain(parents).collect(),
        Err(e) => {
            tracing::warn!(nic = id, error = %e, "the Dante NIC's device path was not read: no [[device]] entry is disabled (dasio#181)");
            Vec::new()
        }
    }
}

/// The present devices of the network adapters' setup class
/// (`CM_Get_Device_ID_List`, class + present filter), read again (a few
/// times) when the list grew between its size and its read.
fn net_ids() -> Result<Vec<String>, String> {
    let w = wide(NET_CLASS_GUID);
    let flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    for _ in 0..LIST_TRIES {
        let mut len = 0u32;
        // SAFETY: an out length and a NUL-terminated filter.
        let cr = unsafe { CM_Get_Device_ID_List_SizeW(&mut len, w.as_ptr(), flags) };
        if cr != CR_SUCCESS {
            return Err(cr_text("CM_Get_Device_ID_List_Size", cr));
        }
        // One more unit than asked: the list always ends in two NULs.
        let mut buf = vec![0u16; len as usize + 1];
        // SAFETY: a NUL-terminated filter and a buffer of `buf.len()` units.
        let cr = unsafe {
            CM_Get_Device_ID_ListW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, flags)
        };
        match cr {
            CR_SUCCESS => return Ok(multi_sz_from_wide(&buf)),
            CR_BUFFER_SMALL => {}
            cr => return Err(cr_text("CM_Get_Device_ID_List", cr)),
        }
    }
    Err(format!(
        "CM_Get_Device_ID_List: still growing after {LIST_TRIES} reads"
    ))
}

/// Every present Net-class device (whatever its address or link), each
/// with its parents; an error when one was not read, or when Windows lists
/// none (a dasio PC has its Dante NIC at least), so nothing is disabled.
pub fn net_adapters() -> Result<Vec<Adapter>, String> {
    let ids = net_ids()?;
    if ids.is_empty() {
        return Err("CM_Get_Device_ID_List: no present network adapter listed".into());
    }
    ids.into_iter()
        .map(|id| match walk_up(&id) {
            Ok(parents) => Ok(Adapter { id, parents }),
            Err(e) => Err(format!("{id}: {e}")),
        })
        .collect()
}

/// The string-list property `key` (`what`) of devnode `dev`, read in one
/// call (a list Windows keeps whole, never walked one handle at a time);
/// empty when the device has none (CR_NO_SUCH_VALUE: a device without
/// children or removal relations answers so, runner 19.11.2026).
fn string_list(dev: u32, key: &DEVPROPKEY, what: &str) -> Result<Vec<String>, String> {
    // Bytes Windows asks for; 2 at first, so the first call gives the size.
    let mut want = 2u32;
    for _ in 0..LIST_TRIES {
        // One more UTF-16 unit than asked: the list ends in two NULs.
        let mut buf = vec![0u16; (want as usize).div_ceil(2) + 1];
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        // SAFETY: a devnode handle from CM_Locate_DevNode, a property key,
        // an out type, and a buffer of `len` bytes.
        let cr = unsafe {
            CM_Get_DevNode_PropertyW(dev, key, &mut kind, buf.as_mut_ptr().cast(), &mut len, 0)
        };
        match cr {
            CR_SUCCESS if kind == DEVPROP_TYPE_STRING_LIST => return Ok(multi_sz_from_wide(&buf)),
            CR_SUCCESS => return Err(format!("{what}: property type {kind}, not a string list")),
            CR_NO_SUCH_VALUE => return Ok(Vec::new()),
            CR_BUFFER_SMALL => want = len,
            cr => return Err(cr_text(what, cr)),
        }
    }
    Err(format!("{what}: still growing after {LIST_TRIES} reads"))
}

/// What stops with `id` when it stops, one step: its children and its
/// removal relations (DEVPKEY_Device_Children, _RemovalRelations); empty
/// for a device not connected.
pub fn removes(id: &str) -> Result<Vec<String>, String> {
    let w = wide(id);
    let mut dev = 0u32;
    // SAFETY: a NUL-terminated instance path and an out handle.
    match unsafe { CM_Locate_DevNodeW(&mut dev, w.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) } {
        CR_SUCCESS => {}
        CR_NO_SUCH_DEVNODE => return Ok(Vec::new()),
        cr => return Err(cr_text("CM_Locate_DevNode", cr)),
    }
    let mut out = string_list(dev, &DEVPKEY_Device_Children, "its children")?;
    out.extend(string_list(
        dev,
        &DEVPKEY_Device_RemovalRelations,
        "its removal relations",
    )?);
    Ok(out)
}

/// The real PC's devices, for `[[device]]`.
impl Devices for Pc {
    fn nic_chain(&self) -> Vec<String> {
        self.nic_chain.clone()
    }

    fn device(&self, id: &str) -> Result<Option<DevState>, String> {
        state(id)
    }

    fn net_adapters(&self) -> Result<Vec<Adapter>, String> {
        net_adapters()
    }

    fn parents(&self, id: &str) -> Result<Vec<String>, String> {
        walk_up(id)
    }

    fn removes(&self, id: &str) -> Result<Vec<String>, String> {
        removes(id)
    }
}

/// The device with instance path `id`; `None` when the PC has no such
/// device.
pub fn state(id: &str) -> Result<Option<DevState>, String> {
    match locate(id)? {
        None => Ok(None),
        Some((dev, present)) => read(dev, present).map(Some),
    }
}

/// Disables `id` now (when connected and on) and at every boot
/// (CONFIGFLAG_DISABLED), then reads it back; the error says what is left.
pub fn disable(id: &str) -> Result<(), String> {
    let Some((dev, present)) = locate(id)? else {
        return Err(format!("device {id}: not on this PC"));
    };
    let s = read(dev, present)?;
    if s.present && s.problem != Some(PROBLEM_DISABLED) {
        // SAFETY: a devnode handle from CM_Locate_DevNode.
        let cr = unsafe { CM_Disable_DevNode(dev, CM_DISABLE_PERSIST | CM_DISABLE_UI_NOT_OK) };
        if cr != CR_SUCCESS {
            return Err(format!(
                "device {id}: {}",
                cr_text("CM_Disable_DevNode", cr)
            ));
        }
    }
    let flags = config_flags(dev)?;
    if flags & CONFIGFLAG_DISABLED == 0 {
        let held = flags | CONFIGFLAG_DISABLED;
        // SAFETY: a 4-byte REG_DWORD buffer and its size.
        let cr = unsafe {
            CM_Set_DevNode_Registry_PropertyW(
                dev,
                CM_DRP_CONFIGFLAGS,
                (&held as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
                0,
            )
        };
        if cr != CR_SUCCESS {
            return Err(format!(
                "device {id}: {}",
                cr_text("setting ConfigFlags", cr)
            ));
        }
    }
    let after = read(dev, present)?;
    if after.off() {
        Ok(())
    } else {
        Err(format!(
            "device {id}: still {} after the disable",
            after.describe()
        ))
    }
}
