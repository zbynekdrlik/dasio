# dante-asio MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Windows PC (ablfoh) runs our own Dante device per instance that a DAW opens as an ASIO driver, with a measured round trip below today's VB-Matrix + AIC128-D path, two instances at once.

**Architecture:** Per instance a device process (`dante-asio run`) hosts the Dante stack copied from iemmixer (`iem-dante`, ported to Windows) and is the clock: its pump runs one block per ASIO buffer and hands the block to the DAW through a shared-memory mapping and a named event. The ASIO driver is one in-process COM DLL registered under N CLSIDs (one per instance); its worker thread waits on the instance's event and calls the host's `bufferSwitchTimeInfo`. A tray icon per instance edits the instance's TOML and restarts its process.

**Tech Stack:** Rust 2024 (workspace), `windows-sys`, `socket2`, `mio`, `tokio`, `mdns-sd` (from iem-dante), `tray-icon` + `muda`, GitHub Actions `windows-latest` and `ubuntu-latest`.

**Spec:** `docs/design/2026-10-29-dante-asio-design.md` (with the recon notes `docs/design/recon-*.md`).

## Global Constraints

- Base: iemmixer's `crates/iem-dante` and the parts of `crates/iem-audio-io` it uses, copied at iemmixer commit `a510531`; nothing else as a base (owner, 2026-10-29).
- License GPL-3.0-or-later (iem-dante is GPL-3.0+).
- No local cargo: GitHub CI builds and tests; Windows behaviour is proven on `windows-latest` and on ablfoh.
- 96 kHz, 24-bit Dante (encoding 24), ASIO sample type `ASIOSTInt32LSB` (18).
- Per instance: its own IPv4 address on the Dante NIC, its own Dante name, its own CLSID and `HKLM\SOFTWARE\ASIO\<name>` key; up to 64 RX and 64 TX.
- dantesync is the only clock owner; the device never opens UDP 319/320.
- Never change another Dante device's subscriptions; tests subscribe our own device to itself.
- Private repo `zbynekdrlik/dasio`; site data (addresses, names) only in the PC's config, never committed.
- Two-branch flow (`dev`, `main`), version in `[workspace.package].version` starting `0.1.0`, dev > main on PRs.

## Review Focus

1. The DAW closes or crashes mid-stream: the device keeps serving Dante, TX goes silent, the next `createBuffers` resumes. Test in Task 6 (`a_host_that_disappears_leaves_the_device_running`).
2. The device process is not running when the DAW opens the driver: `init` succeeds, the driver free-runs silence on its own timer, no hang. Test in Task 6 (`no_device_free_runs_silence`).
3. A peer's closed port makes Windows return `WSAECONNRESET` on `recv_from`: the device must keep serving. Test in Task 2 (`connreset_does_not_end_a_server`).
4. dantesync's daily date-offset step (02:00 UTC, whole 200 ms): the clock must hold, no re-anchor storm. Test in Task 3 (`a_d_step_keeps_ptp_time`).
5. The DAW asks a buffer size other than the instance's block: the driver offers only the configured block (min = max = preferred), so the host cannot pick another. Test in Task 6 (`buffer_size_is_the_instance_block`).

---

### Task 0: The repository

**Files:**
- Create: `Cargo.toml` (workspace, `[workspace.package] version = "0.1.0"`, `edition = "2024"`, `license = "GPL-3.0-or-later"`), `LICENSE`, `README.md`, `CLAUDE.md` (two-branch flow, no local cargo, P6-style rule: no addresses or names in the repo), `.github/workflows/ci.yml`, `scripts/check_version.py` (dev > main), `docs/design/*` (the design and recon files), `docs/plans/2026-10-29-dante-asio-mvp-plan.md` (this plan).

- [ ] **Step 1:** `gh repo create zbynekdrlik/dasio --private`, branches `main` and `dev`.
- [ ] **Step 2:** CI jobs: `fmt` (`cargo fmt --all --check`), `clippy` (`-D warnings`, both OSes), `test-linux` (`cargo test --workspace` minus Windows-only crates), `test-windows` (`cargo test --workspace` on `windows-latest`), `version` (PRs only).
- [ ] **Step 3:** Push `dev`, CI green on an empty workspace member `crates/placeholder` removed in Task 1.
- [ ] **Step 4:** Commit `chore: the dante-asio workspace and CI`.

### Task 1: The copied Dante stack

**Files:**
- Create: `crates/iem-dante/**` (copy of iemmixer `crates/iem-dante` at `a510531`, without `health.rs`, `bin/iem-dante-dev.rs`), `crates/audio-io/src/lib.rs` (copy of `iem-audio-io`'s `Process`, `Block`, `StreamStats`, `panic_message`, `hist`, `telemetry::period_ns` only), `crates/iem-dante/COPIED.md` (the source commit and what was left out).

**Interfaces:**
- Produces: `iem_dante::*` as in iemmixer; `audio_io::{Process, Block, StreamStats}`.

- [ ] **Step 1:** Copy, rewrite `iem_audio_io::` paths to `audio_io::`, drop the health module.
- [ ] **Step 2:** `test-linux` runs the copied tests unchanged; expected all green (they passed in iemmixer at `a510531`).
- [ ] **Step 3:** Commit `feat: copy iem-dante from iemmixer a510531`.

### Task 2: iem-dante on Windows

**Files:**
- Modify: `crates/iem-dante/src/lib.rs` (`host` no longer Linux-only), `host.rs` (control socket and unix signals behind `cfg(unix)`; on Windows `Signals::Report` only; NIC MAC/speed/gateway from `GetAdaptersAddresses`), `media/tx.rs` (`ask_realtime` on Windows: `AvSetMmThreadCharacteristicsW("Pro Audio")` + `AvSetMmThreadPriority(AVRT_PRIORITY_CRITICAL)`; `pin_to_cpu`: `SetThreadAffinityMask`), new `src/winsock.rs` (`no_connreset(&UdpSocket)` sets `SIO_UDP_CONNRESET` false), every UDP socket the device binds or connects calls it, `info.rs` (`set_multicast_if_v4(device ip)` on the info socket).
- Modify: `crates/iem-dante/Cargo.toml` (`windows-sys` features Threading, Media_Multimedia, NetworkManagement_IpHelper, Networking_WinSock, System_Threading).
- Test: `crates/iem-dante/tests/windows.rs` (`#![cfg(windows)]`), `tests/loopback.rs` (gate widened to Linux and Windows).

**Interfaces:**
- Produces: `iem_dante::winsock::no_connreset(sock: &std::net::UdpSocket) -> std::io::Result<()>`; `host::prepare` and `Prepared::start` on Windows.

- [ ] **Step 1: Failing tests** (`tests/windows.rs`):
  - `connreset_does_not_end_a_server`: an ARC server on 127.0.0.1, a request whose reply goes to a closed port (ICMP unreachable), then a second request from a live socket gets its reply within 1 s.
  - `realtime_is_granted`: `ask_realtime(45)` returns `true` on the runner.
  - `loopback` (existing, now on Windows): sample-exact through our own 4455 server on 127.0.0.1.
- [ ] **Step 2:** Push; `test-windows` fails (host gated, connreset).
- [ ] **Step 3:** Implement the changes listed under Files.
- [ ] **Step 4:** Push; all green on both OSes.
- [ ] **Step 5:** Commit `feat(dante): the Dante stack runs on Windows`.

### Task 3: The clock on Windows

**Files:**
- Create: `crates/iem-dante/src/clock/windows.rs` (`WinClock: PtpClock`, `poll_dantesync` over DSYX on 127.0.0.1:31900 with a 256-byte receive buffer).
- Modify: `crates/iem-dante/src/clock.rs` (pick `WinClock` on Windows).
- Test: `crates/iem-dante/src/clock/windows/tests.rs`, portable parts in `clock/tests.rs`.

**Interfaces:**
- Produces: `clock::windows::WinClock::start(stats: Arc<ClockStats>) -> io::Result<Arc<WinClock>>`, PTP ns = `GetSystemTimePreciseAsFileTime` (ns since 1601 converted to Unix ns) − D in effect; D from the DSYX extension; the iemmixer `Tracker`, `STEP_GUARD_NS` and slew rules unchanged (no QPC: it is not rate-steered by dantesync on Windows).

- [ ] **Step 1: Failing tests** with a fake DSYX server on 127.0.0.1 (a test port, injected):
  - `locked_reply_makes_the_clock_ready`: one locked reply with D = 37 s, `now_ns()` within 1 ms of wall − 37 s.
  - `a_d_step_keeps_ptp_time`: D steps by +200 ms announced 5 s ahead; across the step `now_ns()` never jumps more than 1 ms and `epoch` stays 0.
  - `unlocked_reply_is_holdover`: after anchoring, unlocked replies keep the clock ready, `holdovers` counts 1.
- [ ] **Step 2:** Push, red; implement; push, green.
- [ ] **Step 3:** Commit `feat(clock): PTP time from dantesync on Windows`.

### Task 4: Larger blocks

The DAW's buffer is the pump's block (owner: a buffer per instance for a slow DAW).

**Files:**
- Modify: `crates/iem-dante/src/stream.rs` (`BLOCKS: [u64; 5] = [16, 32, 64, 128, 256]`), `device.rs` (config bound and its message `block: 16, 32, 64, 128 or 256 frames`; `work_margin` at least the block).
- Test: `stream/tests.rs` (`every_block_runs_its_schedule`: for each of `BLOCKS`, the pump's output at T + D bit for bit, D = B + P + W), `device/tests.rs` (`block: 48` refused with the message).

- [ ] **Step 1:** Failing tests; push red; implement; push green.
- [ ] **Step 2:** Commit `feat(stream): blocks 16 to 256`.

### Task 5: The shared memory between driver and device

**Files:**
- Create: `crates/shm/src/lib.rs` (layout, pure), `crates/shm/src/windows.rs` (mapping and events).
- Test: `crates/shm/src/tests.rs` (portable), `crates/shm/tests/windows.rs`.

**Interfaces:**
- Produces:
  - `#[repr(C)] pub struct Header { magic: u32 /* "DASI" */, version: u32 /* 1 */, rx: u32, tx: u32, block: u32, rate: u32, seq: AtomicU64 /* blocks the device published */, ack: AtomicU64 /* blocks the host returned */, heartbeat_ms: AtomicU64, device_pid: u32, host_attached: AtomicU32, late: AtomicU64 }`
  - `pub fn size(rx: u32, tx: u32, block: u32) -> usize` (header + 2 halves × (rx + tx) × block × 4 bytes, each buffer 64-byte aligned).
  - `pub fn buffer_offset(half: u32, input: bool, ch: u32, h: &Header) -> usize`.
  - Windows: `Shared::create(instance: u32, layout) -> io::Result<Shared>` (device side, `Local\dante-asio-<n>-shm`, events `Local\dante-asio-<n>-block` and `-ready`), `Shared::open(instance) -> io::Result<Shared>` (driver side), `signal_block()`, `wait_block(timeout_ms) -> bool`, `signal_ready()`, `wait_ready(timeout_ms) -> bool`.
- [ ] **Step 1: Failing tests:** `offsets_are_aligned_and_disjoint` (every buffer 64-aligned, no overlap, for rx/tx in {0, 8, 64} and blocks in `BLOCKS`), Windows `create_then_open_shares_the_buffers` (write in one, read in the other) and `wait_block_times_out`.
- [ ] **Step 2:** Push red; implement; push green.
- [ ] **Step 3:** Commit `feat(shm): the driver/device shared memory`.

### Task 6: The ASIO driver DLL

**Files:**
- Create: `crates/asio-driver/` (`cdylib`): `src/abi.rs` (hand-written IASIO vtable of 21 methods after IUnknown, `#[repr(C, packed(4))]` structs, `ASIOSamples`/`ASIOTimeStamp` as `{hi: u32, lo: u32}`, `extern "system"`), `src/com.rs` (`DllGetClassObject`, `DllCanUnloadNow`, class factory; instance = the CLSID's last byte), `src/driver.rs` (IASIO methods), `src/worker.rs` (MMCSS thread: `wait_block` then `bufferSwitchTimeInfo`; with no device or a heartbeat older than 500 ms it free-runs silence on a high-resolution waitable timer at the block period and sends `kAsioResetRequest` when the device returns).
- Test: `crates/asio-driver/tests/host.rs` (`#![cfg(windows)]`): a test host loads the built DLL, a fake device in the test drives `Shared`.

**Interfaces:**
- Consumes: `shm::Shared`, `shm::Header`.
- Produces: the DLL `dante_asio.dll`; `pub const CLSID_BASE: GUID` (fixed, last byte = instance 1..=16); `QueryInterface` accepts the instance's own CLSID as IID.
- Behaviour: `getChannels` = header rx/tx; `getBufferSize` min = max = preferred = header block, granularity 0; `canSampleRate(96000.0)` only; sample type 18; `getLatencies` input = block, output = block + D (from the header); `outputReady` returns `ASE_OK` and calls `signal_ready`; `future(kAsioCanTimeInfo)` → `ASE_SUCCESS`.

- [ ] **Step 1: Failing tests** (`tests/host.rs`):
  - `abi_layout_matches_the_sdk`: `size_of`/`offset_of` of `ASIODriverInfo`, `ASIOChannelInfo`, `ASIOBufferInfo`, `ASIOTime` equal the SDK's x64 values.
  - `a_block_reaches_the_host_and_back`: fake device writes a ramp into input half 0 and signals; host callback copies input to output and calls `outputReady`; device reads the ramp back from output half 0.
  - `buffer_size_is_the_instance_block`: block 64 → (64, 64, 64, 0).
  - `no_device_free_runs_silence`: no `Shared` → `init` true, `start` OK, at least 10 `bufferSwitch` calls in 100 ms at block 64… (period 0.67 ms) with zeroed inputs.
  - `a_host_that_disappears_leaves_the_device_running`: host disposes without `stop`; fake device keeps publishing, `host_attached` reads 0, next `createBuffers` resumes.
  - `two_instances_are_separate`: CLSIDs 1 and 2 open their own mappings.
- [ ] **Step 2:** Push red; implement; push green.
- [ ] **Step 3:** Commit `feat(asio): the ASIO driver`.

### Task 7: The device process and registration

**Files:**
- Create: `crates/dante-asio/` (bin): `src/main.rs` (subcommands `run --instance <n> --config <toml>`, `register --instance <n> --name <text> --dll <path>`, `unregister --instance <n>`), `src/bridge.rs` (`Bridge: audio_io::Process`: copies the pump's `Block` into the current input half, signals, waits `wait_ready` up to W's time, copies the output half; a missed ready counts `late` and sends silence), `src/register.rs` (registry writes, pure `entries(n, name, dll) -> Vec<(Hive, Key, Value)>` + Windows apply).
- Config: the iemmixer device TOML plus `[instance] number, ip`.
- Test: `src/bridge/tests.rs`, `src/register/tests.rs`, `tests/windows.rs`.

**Interfaces:**
- Consumes: `iem_dante::host::prepare(&Options{place: Place::Ip(ip), ..}, &engine_tx)`, `stream::Pump`, `stream::spawn`, `shm::Shared::create`.
- Produces: `dante-asio.exe run` = one Dante device + the shared memory; `register` writes `HKCR\CLSID\{guid}` (`InprocServer32` = DLL path, `ThreadingModel` = `Apartment`) and `HKLM\SOFTWARE\ASIO\<name>` (`CLSID`, `Description`).

- [ ] **Step 1: Failing tests:** `entries_for_instance_2` (the exact keys and values), `bridge_copies_a_block_through_shm` (fake host acks; output equals input), `bridge_without_host_sends_silence` (no ack within W: `late` 1, outputs zero), Windows `register_then_unregister_round_trip` (on the runner, admin).
- [ ] **Step 2:** Push red; implement; push green.
- [ ] **Step 3:** Commit `feat: the device process and the driver's registration`.

### Task 8: The tray icon

**Files:**
- Create: `crates/dante-asio/src/tray.rs` (`tray-icon` + `muda`, Win32 message loop; one icon per instance; menu: status line (clock, flows, late), RX channels (8/16/32/64), TX channels (8/16/32/64), buffer (16…256), Dante name (opens the TOML in Notepad), Restart; a change writes the TOML and restarts the instance's `run` process), `src/config_edit.rs` (pure TOML edits).
- Test: `src/config_edit/tests.rs` (`set_block_keeps_work_margin_valid`: block 128 lifts `work_margin` to 128; `set_channels_rewrites_names`: 16 RX names `IN 1`..`IN 16`).

- [ ] **Step 1:** Failing tests; push red; implement; push green.
- [ ] **Step 2:** Commit `feat(tray): instance settings in the tray`.

### Task 9: Install on ablfoh

**Files:**
- Create: `scripts/install.ps1` (copy the release's files to `C:\Program Files\dante-asio`, add the instance IP to the Dante NIC with `New-NetIPAddress -SkipAsSource $true`, `register`, a logon scheduled task per instance running `dante-asio run` in the user's session with the tray), `scripts/uninstall.ps1`; CI job `release` builds a zip on `main`.

- [ ] **Step 1:** Pick the instance address: free in the Dante subnet (no ARP reply, no ping, not in Dante discovery), recorded only on the PC.
- [ ] **Step 2:** Uninstall Dante Virtual Soundcard (owner, 2026-10-29); record which services stopped.
- [ ] **Step 3:** Install instance 1 at 8 RX / 8 TX, block 32. Verify: Dante Controller (netaudio from the dev box) lists the device; `HKLM\SOFTWARE\ASIO` lists it; the tray icon shows.

### Task 10: Prove it on ablfoh

**Files:**
- Create: `crates/dante-asio/src/probe.rs` (`dante-asio probe --instance <n> --seconds <s>`: an ASIO host that opens the driver, loops input→output inside the callback, emits an impulse on TX 1 each second, reports the round trip from RX 1 in samples and the late/xrun counts).

- [ ] **Step 1:** Subscribe instance 1's RX 1 to its own TX 1 (our own device only).
- [ ] **Step 2:** `probe` 60 s at blocks 32, 64, 128: round trip in samples, late 0. Record.
- [ ] **Step 3:** Today's path for comparison, measured without changing any device: Ableton's reported latencies (VB-Matrix + AIC128-D) from its preferences and the drivers' `getLatencies`, plus Dante's flow latency read from the AIC128-D (ARC, read-only).
- [ ] **Step 4:** Ableton 12 (the band instance's copy of the set, not the live set) on instance 1: select the driver, play 10 minutes, log from the driver whether Live uses `outputReady` and `bufferSwitchTimeInfo`.
- [ ] **Step 5:** One hour of `probe`: late 0, no clock re-anchor, no missing packet.
- [ ] **Step 6:** Instance 2 at the same time (second address, second CLSID), both `probe`s 10 minutes clean.
- [ ] **Step 7:** Record the numbers in the repo (`docs/results/2026-10-xx-mvp.md`) and report to the owner.
