// SPDX-License-Identifier: GPL-3.0-or-later
// The driver runs inside the DAW: outside its tests it never unwraps, never
// expects and never panics on purpose (dasio#28, E14); every export is
// also guarded (`guard`).
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

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
pub mod systime;
pub mod timeline;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod driver;

#[cfg(windows)]
pub use com::{DllCanUnloadNow, DllGetClassObject};
