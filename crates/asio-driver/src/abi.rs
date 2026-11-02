//! The ASIO ABI as the Steinberg SDK 2.3.4's `asio.h` and `iasiodrv.h`
//! define it, written out (x64 only): `#pragma pack(4)` structs, 64-bit
//! values as `{hi, lo}` pairs, `long` as `i32`, every IASIO method with
//! `this` first (`extern "system"`, which is the one x64 convention).

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::{c_char, c_void};

pub type ASIOBool = i32;
pub type ASIOError = i32;
pub type HRESULT = i32;

pub const ASIO_TRUE: ASIOBool = 1;
pub const ASIO_FALSE: ASIOBool = 0;

pub const ASE_OK: ASIOError = 0;
pub const ASE_SUCCESS: ASIOError = 0x3f48_47a0;
pub const ASE_NOT_PRESENT: ASIOError = -1000;
pub const ASE_HW_MALFUNCTION: ASIOError = -999;
pub const ASE_INVALID_PARAMETER: ASIOError = -998;
pub const ASE_INVALID_MODE: ASIOError = -997;
pub const ASE_SP_NOT_ADVANCING: ASIOError = -996;
pub const ASE_NO_CLOCK: ASIOError = -995;
pub const ASE_NO_MEMORY: ASIOError = -994;

/// `ASIOSTInt32LSB`: 32-bit little-endian integer (our 24 bits left-justified).
pub const ASIOST_INT32_LSB: i32 = 18;

// asioMessage selectors.
pub const K_ASIO_SELECTOR_SUPPORTED: i32 = 1;
pub const K_ASIO_ENGINE_VERSION: i32 = 2;
pub const K_ASIO_RESET_REQUEST: i32 = 3;
pub const K_ASIO_BUFFER_SIZE_CHANGE: i32 = 4;
pub const K_ASIO_RESYNC_REQUEST: i32 = 5;
pub const K_ASIO_LATENCIES_CHANGED: i32 = 6;
pub const K_ASIO_SUPPORTS_TIME_INFO: i32 = 7;
pub const K_ASIO_SUPPORTS_TIME_CODE: i32 = 8;
pub const K_ASIO_OVERLOAD: i32 = 15;

// future selectors.
pub const K_ASIO_CAN_TIME_INFO: i32 = 10;
pub const K_ASIO_CAN_TIME_CODE: i32 = 11;

// AsioTimeInfo flags.
pub const K_SYSTEM_TIME_VALID: u32 = 1;
pub const K_SAMPLE_POSITION_VALID: u32 = 1 << 1;
pub const K_SAMPLE_RATE_VALID: u32 = 1 << 2;
pub const K_SPEED_VALID: u32 = 1 << 3;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const IID_IUNKNOWN: Guid = Guid {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};
pub const IID_ICLASSFACTORY: Guid = Guid {
    data1: 0x0000_0001,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};

/// A 64-bit value as the SDK passes it on Windows: high, then low.
#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy, Default)]
pub struct Asio64 {
    pub hi: u32,
    pub lo: u32,
}

impl Asio64 {
    pub fn new(v: u64) -> Self {
        Asio64 {
            hi: (v >> 32) as u32,
            lo: v as u32,
        }
    }

    /// The value, as a host reads it.
    pub fn get(self) -> u64 {
        (u64::from(self.hi) << 32) | u64::from(self.lo)
    }
}

pub type ASIOSamples = Asio64;
pub type ASIOTimeStamp = Asio64;

#[repr(C, packed(4))]
pub struct ASIOClockSource {
    pub index: i32,
    pub associatedChannel: i32,
    pub associatedGroup: i32,
    pub isCurrentSource: ASIOBool,
    pub name: [c_char; 32],
}

#[repr(C, packed(4))]
pub struct ASIOChannelInfo {
    pub channel: i32,
    pub isInput: ASIOBool,
    pub isActive: ASIOBool,
    pub channelGroup: i32,
    pub type_: i32,
    pub name: [c_char; 32],
}

#[repr(C, packed(4))]
pub struct ASIOBufferInfo {
    pub isInput: ASIOBool,
    pub channelNum: i32,
    pub buffers: [*mut c_void; 2],
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct AsioTimeInfo {
    pub speed: f64,
    pub systemTime: ASIOTimeStamp,
    pub samplePosition: ASIOSamples,
    pub sampleRate: f64,
    pub flags: u32,
    pub reserved: [c_char; 12],
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct ASIOTimeCode {
    pub speed: f64,
    pub timeCodeSamples: ASIOSamples,
    pub flags: u32,
    pub future: [c_char; 64],
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct ASIOTime {
    pub reserved: [i32; 4],
    pub timeInfo: AsioTimeInfo,
    pub timeCode: ASIOTimeCode,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ASIOCallbacks {
    pub bufferSwitch: Option<unsafe extern "C" fn(i32, ASIOBool)>,
    pub sampleRateDidChange: Option<unsafe extern "C" fn(f64)>,
    pub asioMessage: Option<unsafe extern "C" fn(i32, i32, *mut c_void, *mut f64) -> i32>,
    pub bufferSwitchTimeInfo:
        Option<unsafe extern "C" fn(*mut ASIOTime, i32, ASIOBool) -> *mut ASIOTime>,
}

/// The IASIO vtable: IUnknown's three, then the SDK's 21 in order.
#[repr(C)]
pub struct IAsioVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub init: unsafe extern "system" fn(*mut c_void, *mut c_void) -> ASIOBool,
    pub getDriverName: unsafe extern "system" fn(*mut c_void, *mut c_char),
    pub getDriverVersion: unsafe extern "system" fn(*mut c_void) -> i32,
    pub getErrorMessage: unsafe extern "system" fn(*mut c_void, *mut c_char),
    pub start: unsafe extern "system" fn(*mut c_void) -> ASIOError,
    pub stop: unsafe extern "system" fn(*mut c_void) -> ASIOError,
    pub getChannels: unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32) -> ASIOError,
    pub getLatencies: unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32) -> ASIOError,
    pub getBufferSize:
        unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32, *mut i32, *mut i32) -> ASIOError,
    pub canSampleRate: unsafe extern "system" fn(*mut c_void, f64) -> ASIOError,
    pub getSampleRate: unsafe extern "system" fn(*mut c_void, *mut f64) -> ASIOError,
    pub setSampleRate: unsafe extern "system" fn(*mut c_void, f64) -> ASIOError,
    pub getClockSources:
        unsafe extern "system" fn(*mut c_void, *mut ASIOClockSource, *mut i32) -> ASIOError,
    pub setClockSource: unsafe extern "system" fn(*mut c_void, i32) -> ASIOError,
    pub getSamplePosition:
        unsafe extern "system" fn(*mut c_void, *mut ASIOSamples, *mut ASIOTimeStamp) -> ASIOError,
    pub getChannelInfo: unsafe extern "system" fn(*mut c_void, *mut ASIOChannelInfo) -> ASIOError,
    pub createBuffers: unsafe extern "system" fn(
        *mut c_void,
        *mut ASIOBufferInfo,
        i32,
        i32,
        *const ASIOCallbacks,
    ) -> ASIOError,
    pub disposeBuffers: unsafe extern "system" fn(*mut c_void) -> ASIOError,
    pub controlPanel: unsafe extern "system" fn(*mut c_void) -> ASIOError,
    pub future: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> ASIOError,
    pub outputReady: unsafe extern "system" fn(*mut c_void) -> ASIOError,
}

/// IClassFactory's vtable.
#[repr(C)]
pub struct IClassFactoryVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub CreateInstance: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const Guid,
        *mut *mut c_void,
    ) -> HRESULT,
    pub LockServer: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

pub const S_OK: HRESULT = 0;
pub const S_FALSE: HRESULT = 1;
pub const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
pub const E_POINTER: HRESULT = 0x8000_4003_u32 as i32;
pub const E_UNEXPECTED: HRESULT = 0x8000_FFFF_u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HRESULT = 0x8004_0111_u32 as i32;
pub const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110_u32 as i32;

#[cfg(test)]
mod tests {
    #[test]
    fn a_64_bit_value_reads_back_as_written() {
        for v in [0, 1, 0x1_0000_0000, 0x1234_5678_9abc_def0, u64::MAX] {
            assert_eq!(super::Asio64::new(v).get(), v);
        }
    }

    use super::*;

    /// The values the ASIO SDK's asio.h defines: a DAW compares against
    /// these numbers, not our names.
    #[test]
    fn the_constants_are_the_sdk_s() {
        assert_eq!(
            [
                ASE_NOT_PRESENT,
                ASE_HW_MALFUNCTION,
                ASE_INVALID_PARAMETER,
                ASE_INVALID_MODE,
                ASE_SP_NOT_ADVANCING,
                ASE_NO_CLOCK,
                ASE_NO_MEMORY,
            ],
            [-1000, -999, -998, -997, -996, -995, -994]
        );
        assert_eq!(ASE_SUCCESS, 0x3f48_47a0);
        assert_eq!(
            [
                K_SYSTEM_TIME_VALID,
                K_SAMPLE_POSITION_VALID,
                K_SAMPLE_RATE_VALID,
                K_SPEED_VALID
            ],
            [1, 2, 4, 8]
        );
    }
}
