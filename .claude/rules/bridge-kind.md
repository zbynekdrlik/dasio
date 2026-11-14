---
paths:
  - "crates/dasio/src/kind.rs"
  - "crates/dasio/src/kind/**"
  - "crates/dasio/src/run_bridge.rs"
  - "crates/dasio/src/place.rs"
---

# The bridge instance kind (dasio#106)

- A `[bridge]` table in `device.toml` makes an instance a bridge to a local
  ASIO driver (`driver`, `to_asio`, `from_asio`, optional `app_buffer_ns`;
  channels 1-based, one per RX / TX channel). No table: the DAW kind.
- dasio's own keys (`iface`, `[bridge]`) are removed in
  `place::device_config` before dstack's `DeviceConfig` reads the file
  (deny_unknown_fields). A new dasio key goes there too, and dtray's
  `Fields::read` (dstack) must learn it, or the settings window refuses the
  file.
- A bridge instance spawns no child process (SoundGrid resyncs on a spawn):
  never call `run::profile` or `Command::new` from `run_bridge.rs`.
- The runner thread makes `WinDevice::new()` itself: the device is `!Send`
  (its COM STA lives on that thread). `Bridge` is shared as
  `Arc<Mutex<Bridge>>`: `MakeProcess` calls `asio_side` there, the main
  loop calls `collect()` every 100 ms.
- A parked driver (`Reason::Parked`) never opens again in the process: the
  instance ends for the SCM to restart it.
- `run_bridge.rs` and `run/profile.rs` are Windows glue, excluded from
  mutants; keep decisions in `kind.rs` / `app_status.rs`, tested on Linux.
