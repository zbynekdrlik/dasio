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
  never call `Command::new` from `run_bridge.rs`, nor `ProfileWatch::start`
  (the DAW kind's `dasio pc-profile check` child). The bridge checks the PC
  profile with `ProfileWatch::start_in_process` (dasio#120): its own thread
  at the lowest priority, `pc_profile::in_process` drops `[[bcd]]` and
  `[[task]]` (bcdedit / schtasks are child processes); the boot task's
  `pc-profile apply` covers those.
- The runner thread makes `WinDevice::new()` itself: the device is `!Send`
  (its COM STA lives on that thread). `Bridge` is shared as
  `Arc<Mutex<Bridge>>`: `MakeProcess` calls `asio_side` there, the main
  loop calls `collect()` every 100 ms.
- A parked driver (`Reason::Parked`) never opens again in the process: the
  instance ends for the SCM to restart it.
- `run_bridge.rs` and `run/profile.rs` are Windows glue, excluded from
  mutants; keep decisions in `kind.rs` / `app_status.rs`, tested on Linux.
- The from-ASIO figures (dstack issue 325, dasio issue 120, dstack
  v0.6.151): `bridge_json` writes each of `from_asio_block_age_us`,
  `from_asio_block_age_max_us` (the modelled block age, a diagnostic only),
  `from_asio_packet_age_us` (the TX packets' measured peak age past their
  stamps over the last minute) and `from_asio_min_receiver_latency_us`
  (packet age + the network p99, or 200 us while none is measured: always
  on Windows) only while that value is Some. dsg's field read (16.11.2026)
  had the modelled receiver figure at 2222 us (block age max 1888 us + a
  pump block) against real packet ages of 1003-1397 us: judge a
  receiver by the packet keys, never the block keys.
- `app_status::from_asio_age_tick(&age, &media.tx.take_app_view(),
  media.rx.network_p99_ns())` runs once in `run_bridge.rs` inside the
  `counts.due(now)` branch (10 s), never the pump or the callback (the view
  locks the TX table), with or without a driver serving (only a look
  records the packet age, so it is never gated on `report()`). It is the
  device's only `take_app_view` reader: a second one takes its peak. The
  hourly WARN ("a receiver counted our TX packets late") comes only from a
  flow's own `over_latency` rising, not from a threshold. A test reads
  `run_bridge.rs` and fails if the call or the view moves or is duplicated.
- The to-ASIO side is pulled (dasio issue 120, dstack issue 307 step 2,
  dstack v0.6.155): `run_bridge.rs` claims `media.rx.app_intake()` before
  `Pump::new` (the pump would claim the rings otherwise; `None` = another
  writer, the start fails), builds `RxPull::new(..)` (returns
  `Result<_, String>`), hands it over with `Bridge::pull_to_asio` before
  `spawn_runner` (refused after the driver's first open), and gives the
  pump `Map { rx: vec![], .. }`. `Bridge::keep_pulled()` runs on the loop's
  own `Every` of `to_asio_pull::KEEP_EVERY` (250 ms): never the 10 s counts
  tick (the idle drain must keep the flow within its 2 s) and never every
  100 ms pass (a driver buffer over 100 ms would read as stopped). What a
  look logs is `to_asio_pull::notes` (portable, tested); its tests also
  guard this order in `run_bridge.rs` as text. The counts line carries
  `to_asio_pulls`, `to_asio_pull_restarts`, `to_asio_idle_drains`.
