---
paths:
  - "crates/dasio/src/kind.rs"
  - "crates/dasio/src/kind/**"
  - "crates/dasio/src/run_bridge.rs"
  - "crates/dasio/src/place.rs"
---

# The bridge instance kind (dasio#106)

- A `[bridge]` table in `device.toml` makes an instance a bridge to a local
  ASIO driver (`driver`, `to_asio`, `from_asio`, optional `app_buffer_ns`,
  `to_asio_app_buffer_ns`, `from_asio_app_buffer_ns`; channels 1-based, one
  per RX / TX channel). No table: the DAW kind.
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
  dstack v0.6.155): `run_bridge::pull_to_driver` (called before
  `Pump::new`, which would claim the rings otherwise) claims
  `media.rx.app_intake()` (`None` = another writer, the start fails),
  builds `RxPull::new(..)` (returns `Result<_, String>`) and hands it over
  with `Bridge::pull_to_asio` before `spawn_runner` (refused after the
  driver's first open); the pump gets `Map { rx: vec![], .. }`.
  `run_bridge::look_at_pull` (`Bridge::keep_pulled()`) runs on the loop's
  own `Every` of `to_asio_pull::KEEP_EVERY` (250 ms): never the 10 s counts
  tick (the idle drain must keep the flow within its 2 s) and never every
  100 ms pass (a driver buffer over 100 ms would read as stopped). What a
  look logs is `to_asio_pull::notes` (portable, tested; the start's idle
  before any pull is INFO, a driver that stopped pulling WARN); its tests
  also guard this wiring in `run_bridge.rs` as text. The counts line
  carries `to_asio_pulls`, `to_asio_pull_restarts`, `to_asio_idle_drains`.
- Pulled, the RX packets are placed at the driver's buffer switch, not
  every pump block. Up to dstack v0.6.156 dstack's lateness (dasio's
  `rx_arrival_*`, dtray's "RX arrival", dcontroller's latency graph for a
  bridge device) included up to one driver buffer period of that wait.
  Since v0.6.157 (dstack issue 350) it is the arrival age again (Windows,
  no receive stamps: bracketed by the reads, a late burst just before a
  read shows in `lateness_at_read_peak`), and the wait is the bridge's
  `rx_asio_pull_wait_mean_ns` / `rx_asio_pull_wait_peak_us` (dasio#128).
  `late` is judged against the drain gap either way.
- The from-ASIO side is pulled too (dasio issue 120, dstack issue 364,
  dstack v0.6.167): `run_bridge::pull_from_driver` (called before
  `dante_side()`, so before `Pump::new`, the device's start and the
  runner's first open) takes `Bridge::pull_from_asio()` and claims it as
  `media.tx.claim_fill(TxFill::new(Box::new(from), engine_tx, timing,
  cfg.encoding, rx))` (the pump's own timing; a `Refused` is a start
  error, no pump fallback). The pump's map is `Map { rx: vec![], tx:
  vec![] }` (one writer a ring). A bridge without from-ASIO channels skips
  it (`pull_from_asio` refuses one). The fill's counts (dstack status
  `tx_pull`) are the from-ASIO side's health: `bridge_json`'s
  `from_asio_pull` object (only with channels from ASIO), the line's
  ` · from ASIO pull: restarts N, skipped N, late writes N, faults N` (only
  when one is above 0) and a warn icon only on a fault (the processor
  panicked: silent for good). `from_asio_block_age_us` is now taken at the
  TX thread's fill. A refused second driver open reads `the Dante device
  has not taken the last output yet` (was "...pump has not taken...").
  The counts line carries `from_asio_pull` (`None` without a pull).
  `app_status/tests.rs` guards this wiring as text.
- A text guard over `run_bridge.rs` (`include_str!`) whose needle spans a
  line break must normalise `\r\n` first: the Windows CI checkout has CRLF
  lines, so a `"...\n        ..."` needle passes on Linux and fails the
  Windows test and coverage jobs (PR 169, run 44055706009).
- `app_buffer_ns` is ONE target for both directions (dstack-asio
  `Spec.app_buffer_ns`; default = driver buffer + one pump block, 1.667 ms
  on dsg's MiniFuse 128 @ 96 kHz). dsg's 17.11.2026 steps (dasio#120):
  1.333 ms held to-ASIO (0 underruns, 59 min, late callbacks included)
  but not from-ASIO (4 underruns in 29 min); 1.0 ms failed both ways
  (to-ASIO once in the ASRC's first unlocked minute, from-ASIO 4 in 6
  min). A lower bridge latency needs dstack (per-direction targets, the
  from-ASIO converter call at the TX block, the sinc length), not a
  smaller value here. The `rx_asio_pull_wait` is no latency term (the
  read is by position, media - P).
- Since dstack v0.6.172 (dstack issue 382) each direction takes its own
  target: `[bridge] to_asio_app_buffer_ns` / `from_asio_app_buffer_ns`
  (unset: `app_buffer_ns`, unset: the default), checked by `kind::buffer_ns`
  against dlink's range under the key's own name. dtray's `Fields::read`
  (dstack issue 387) learns these key names.
- The ASRC seed (dstack v0.6.172): `asrc_seed::path` = the instance
  folder's `asrc-seed-<driver>.toml` (driver name kept to `[A-Za-z0-9_-]`),
  `Seed::read` + `Bridge::seed_from` before `spawn_runner`, and
  `save_seed` (via `asrc_seed::Keeper`: only a changed seed, a failed write
  retried, its error logged once) on the loop's 60 s `save` timer and once
  after the device stops. `asrc_seed/tests.rs` guards this wiring as text.
