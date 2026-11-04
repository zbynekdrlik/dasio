//! The COM in-process server: `DllGetClassObject` hands a class factory for
//! a registered instance's CLSID; the factory makes that instance's driver.
//! An ASIO host passes the CLSID as the interface id too, so the driver
//! answers its own CLSID (and IUnknown). Methods that build objects run in
//! [`guard`]: a panic never unwinds into the host.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::abi::*;
use crate::driver::Driver;
use crate::guard::guard;
use crate::ident::instance_of;

/// Live objects (factories and drivers) and server locks.
pub(crate) static OBJECTS: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Factory {
    vtbl: *const IClassFactoryVtbl,
    refs: AtomicU32,
    instance: u32,
}

static FACTORY_VTBL: IClassFactoryVtbl = IClassFactoryVtbl {
    QueryInterface: factory_qi,
    AddRef: factory_add_ref,
    Release: factory_release,
    CreateInstance: factory_create,
    LockServer: factory_lock,
};

/// # Safety
/// `rclsid`, `riid` and `ppv` are valid pointers, as COM passes them.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const Guid,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    guard(E_UNEXPECTED, || {
        if ppv.is_null() || rclsid.is_null() || riid.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null; COM's pointers.
        unsafe { *ppv = core::ptr::null_mut() };
        // SAFETY: as above.
        let Some(instance) = instance_of(unsafe { &*rclsid }) else {
            return CLASS_E_CLASSNOTAVAILABLE;
        };
        // SAFETY: as above.
        let iid = unsafe { *riid };
        if iid != IID_ICLASSFACTORY && iid != IID_IUNKNOWN {
            return E_NOINTERFACE;
        }
        let f = Box::new(Factory {
            vtbl: &FACTORY_VTBL,
            refs: AtomicU32::new(1),
            instance,
        });
        OBJECTS.fetch_add(1, Ordering::AcqRel);
        // SAFETY: checked non-null.
        unsafe { *ppv = Box::into_raw(f).cast() };
        S_OK
    })
}

/// S_OK once no object lives and no lock is held.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::Acquire) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

unsafe extern "system" fn factory_qi(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM's valid pointers.
    let iid = unsafe { *riid };
    if iid == IID_ICLASSFACTORY || iid == IID_IUNKNOWN {
        // SAFETY: `this` is a live Factory.
        unsafe {
            factory_add_ref(this);
            *ppv = this;
        }
        S_OK
    } else {
        // SAFETY: checked non-null.
        unsafe { *ppv = core::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn factory_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live Factory.
    let f = unsafe { &*(this as *const Factory) };
    f.refs.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn factory_release(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live Factory.
    let left = unsafe { &*(this as *const Factory) }
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1;
    if left == 0 {
        // SAFETY: the last reference: the Box this factory came from.
        drop(unsafe { Box::from_raw(this as *mut Factory) });
        OBJECTS.fetch_sub(1, Ordering::AcqRel);
    }
    left
}

unsafe extern "system" fn factory_create(
    this: *mut c_void,
    outer: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    guard(E_UNEXPECTED, || {
        if ppv.is_null() || riid.is_null() {
            return E_POINTER;
        }
        // SAFETY: checked non-null.
        unsafe { *ppv = core::ptr::null_mut() };
        if !outer.is_null() {
            return CLASS_E_NOAGGREGATION;
        }
        // SAFETY: `this` is a live Factory.
        let instance = unsafe { &*(this as *const Factory) }.instance;
        // SAFETY: COM's valid pointer.
        let iid = unsafe { *riid };
        if iid != IID_IUNKNOWN && instance_of(&iid) != Some(instance) {
            return E_NOINTERFACE;
        }
        // SAFETY: checked non-null.
        unsafe { *ppv = Driver::new_raw(instance) };
        S_OK
    })
}

unsafe extern "system" fn factory_lock(_this: *mut c_void, lock: i32) -> HRESULT {
    if lock != 0 {
        OBJECTS.fetch_add(1, Ordering::AcqRel);
    } else {
        OBJECTS.fetch_sub(1, Ordering::AcqRel);
    }
    S_OK
}
