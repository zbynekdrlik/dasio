// SPDX-License-Identifier: GPL-3.0-or-later

//! dasio's ASIO driver (plan Task 6): one DLL, registered under one
//! CLSID per instance ([`ident`]); each instance's audio goes through its
//! device process's shared memory (`shm`). The device is the clock: the
//! driver's worker thread waits for each published block and calls the
//! DAW's `bufferSwitchTimeInfo`. With no device it free-runs silence on its
//! own timer, so the DAW never hangs, and asks the DAW for a reset once the
//! device is there.

pub mod abi;
pub mod guard;
pub mod ident;
pub mod liveness;
pub mod reset;
pub mod timeline;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod driver;

#[cfg(windows)]
pub use com::{DllCanUnloadNow, DllGetClassObject};
