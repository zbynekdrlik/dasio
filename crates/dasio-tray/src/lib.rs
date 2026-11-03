// SPDX-License-Identifier: GPL-3.0-or-later

//! The dasio tray: an icon whose colour says how every installed instance
//! is doing, and a menu with one line per instance (its service, its DAW,
//! its block and latencies, its late blocks). It reads each instance's
//! header read-only ([`shm::peek`]) and the service manager; it changes
//! nothing. Its Exit ends the tray only.

pub mod icon;
pub mod view;
