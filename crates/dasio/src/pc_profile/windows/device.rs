//! Reading and disabling one device through the configuration manager
//! (cfgmgr32, dasio#181): the Windows glue of
//! [`device`](super::super::device). No process is started, so the
//! bridge's in-process check reads devices too.

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DISABLE_PERSIST, CM_DISABLE_UI_NOT_OK, CM_DRP_CONFIGFLAGS, CM_Disable_DevNode,
    CM_Get_DevNode_Registry_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_IDW, CM_Get_Parent,
    CM_LOCATE_DEVNODE_NORMAL, CM_LOCATE_DEVNODE_PHANTOM, CM_Locate_DevNodeW,
    CM_Set_DevNode_Registry_PropertyW, CONFIGFLAG_DISABLED, CONFIGRET, CR_NO_SUCH_DEVNODE,
    CR_NO_SUCH_VALUE, CR_REMOVE_VETOED, CR_SUCCESS, DN_HAS_PROBLEM, MAX_DEVICE_ID_LEN,
};

use super::super::device::{DevState, PROBLEM_DISABLED};
use super::{from_wide, wide};

/// The most parents walked from the Dante NIC: far more than any device
/// tree has between a NIC and its root.
const MAX_DEPTH: usize = 64;

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

/// The Dante NIC's instance path `id`, then each of its parents' up to the
/// root; empty when the NIC or a parent was not read (then `[[device]]`
/// disables nothing).
pub fn chain(id: &str) -> Vec<String> {
    let walk = || -> Result<Vec<String>, String> {
        let (mut dev, _) = locate(id)?.ok_or("not found")?;
        let mut out = vec![id.to_string()];
        for _ in 0..MAX_DEPTH {
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
    };
    match walk() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(nic = id, error = %e, "the Dante NIC's device path was not read: no [[device]] entry is disabled (dasio#181)");
            Vec::new()
        }
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
