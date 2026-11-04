# iemmixer lessons for dante-asio (Windows ASIO, realtime, the PCs)

Read-only research, 2026-10-30. Sources: iemmixer `.claude/rules/{asio-spike,guard,pc-tuning,engine}.md`,
iemmixer `crates/iem-audio-io/src/{asio/*,rtpanic,reset,period,owner,os,cpuset,messages}.rs`,
`crates/iem-win/src/{errmode,power}.rs`, iemmixer issues #3, #9, #10, #15, #35, #52, #72;
dante-asio issue #1 for context. Compared with dante-asio `dev` at `1dd4c0e`.

**Key difference to keep in mind.** iemmixer is an ASIO *host* (its own exe, it owns the
process). dante-asio's `asio-driver` is a DLL *inside the DAW's process*. Several iemmixer
mechanisms (process-wide SEH filter, process-wide panic hook, `SetErrorMode`, process priority
class, `VirtualLock` of the working set) must NOT be copied into the DLL as-is: they would change
Ableton's process. They map to the device process (`crates/dante-asio`), or to a DLL-local form
(catch at every exported entry). Each lesson below says which side it belongs to.

Site data (addresses, device and channel names, CPU numbers of the venue) is deliberately left out.

---

## 1. Lessons

### ASIO protocol and driver behaviour

- **The ASIO SDK latencies are only valid after `createBuffers`.** Values read before it were
  nonsense (millions of samples) on the real card. Hosts behave the same way, so a driver must
  return correct values once buffers exist, and may be asked before.
  Source: iemmixer#3 https://github.com/zbynekdrlik/iemmixer/issues/3#issuecomment-5852652207 (probe section).
  dante-asio: **yes** — `get_latencies` reads the header's values any time (crates/asio-driver/src/driver.rs:138-153, 326-342); verify Ableton re-reads them after `kAsioResetRequest`.

- **A driver re-reads its preferred buffer at open; changing it while open makes the driver ask for a
  reset, and a reset at each reopen loops.** The first engine run faulted on its reopen budget about
  110 ms after open because the preference was written back while the driver was open.
  Source: iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5866386905 and #issuecomment-5867716656; `.claude/rules/guard.md` "Preference window".
  dante-asio: **partly** — the buffer is fixed per instance (min=max=preferred, driver.rs:344-365) so the DAW cannot change it, but a tray change of `block` while a DAW holds the buffers has no defined path. Rule to adopt: a block change restarts the device and the driver sends exactly one `kAsioResetRequest`; never a stream of them.

- **The configured buffer is a request, not a fact: measure the delivered period from the first
  callbacks' sample positions and refuse anything else.** (`owner::open_period`, `period::measured`:
  8 agreeing deltas decide.)
  Source: `.claude/rules/asio-spike.md` line 21; iemmixer `crates/iem-audio-io/src/period.rs`.
  dante-asio: **no** — the driver hands blocks by `seq` and never checks that the device's
  published block equals the shape the DAW opened (driver.rs:611-631). Add a check that `seq`
  advances by exactly 1 and count gaps (a `seq` jump > 1 = missed blocks for the DAW, now invisible).

- **Reopen / reset budget.** At most one reopen per 5 min and three per process; a stall is no callback
  for 2 s; over budget = fault and let the supervisor restart. Without a budget a misbehaving
  peer causes a reset storm.
  Source: iemmixer `crates/iem-audio-io/src/reset.rs` (head comment); `.claude/rules/asio-spike.md` line 21.
  dante-asio: **no** — `kAsioResetRequest` is sent once per free-run → device-found transition (driver.rs:644-649), with no rate limit; a device that flaps (starts, dies, starts) makes the DAW reset every second.

- **Never call `set_sample_rate`, `set_clock_source` or `open_control_panel` on a foreign driver**
  (integrity scan bans them in iemmixer). For dante-asio as the *driver*, the mirror rule: reject any
  rate but 96 kHz and expose exactly one clock source.
  Source: `.claude/rules/asio-spike.md` line 22; iemmixer#3 https://github.com/zbynekdrlik/iemmixer/issues/3#issuecomment-5853108933.
  dante-asio: **yes** — driver.rs:367-415 (`set_sample_rate` accepts 0.0 too, a common host probe; fine).

- **Outputs zeroed before `start()` and on every callback, and every unused output gets a zero buffer**
  (A1: never play unknown audio into the band's channels).
  Source: `.claude/rules/asio-spike.md` line 22; iemmixer#3 #issuecomment-5852652207.
  dante-asio: **partly** — the device sends silence when `ack` is late (crates/dante-asio/src/bridge.rs:60-68) and RX is zeroed on quiet (driver.rs:686-694), but the TX halves of the mapping are not cleared at `createBuffers`/`start`, so the first block after a DAW reattach can carry stale samples from the previous session.

- **Driver messages and rate-change callbacks may arrive on any thread; only count them in atomics,
  log them from a non-RT thread.**
  Source: `.claude/rules/asio-spike.md` line 21 ("Logging stays out of the callback"); iemmixer `messages.rs`.
  dante-asio: **n/a for the driver side** (dante-asio is the one *sending* messages). Mirror: the worker calls the DAW's `asioMessage` (driver.rs:696-701); make sure it is never called from inside `bufferSwitch` re-entrantly and log how many were sent.

- **`azo-sys` `I64Split` lacked `#[repr(C)]`** (worked by luck). Layout of every ABI struct must be pinned.
  Source: iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5855814490.
  dante-asio: **yes** — hand-written ABI with a layout test (crates/asio-driver/tests/host.rs `abi_layout_matches_the_sdk_x64`).

### Realtime discipline in the audio path

- **The default Rust panic hook allocates and locks stderr: a panic in the callback cost 4.29 ms,
  one overrun, one missed period, one position gap.** iemmixer installs a hook that, on a thread marked
  RT, only stores file/line/count in atomics; the panic payload is never freed on the RT thread.
  Source: iemmixer#3 https://github.com/zbynekdrlik/iemmixer/issues/3#issuecomment-5855809080 (Fault run); `crates/iem-audio-io/src/rtpanic.rs`; `asio/backend.rs` render.
  dante-asio: **no** in the driver; device side depends on `audio_io`/dante-stack `Pump` (has `catch_unwind` per iemmixer engine.md). In the DLL a process-wide hook is not ours to set: use a `catch_unwind` at each entry instead (see §2).

- **The callback path allocates, locks, logs and makes syscalls never** (I7), proven by `assert_no_alloc`
  tests, rtsan on nightly, and a bench with p50/worst gates.
  Source: `.claude/rules/engine.md` (RT contract I7).
  dante-asio: **partly** — the worker's hot loop is mostly atomics, but: `output_ready` takes `rt.shm`'s `Mutex` on the DAW's audio thread (driver.rs:562); `Job::run` locks the same mutex at start (588); the free-run path calls `Shared::open` every second from the RT thread (646, a syscall + allocation); `get_channel_info` uses `format!` (452, not RT but on the host thread). No no-alloc or rtsan proof exists.

- **Process priority: iemmixer ran HIGH_PRIORITY_CLASS, power throttling off (EXECUTION_SPEED and
  IGNORE_TIMER_RESOLUTION), the RT buffers `VirtualLock`ed after growing the working set (+64 MiB),
  never REALTIME_PRIORITY_CLASS for the engine** (TIME_CRITICAL only on the hwlat scanner).
  The ASIO callback thread is the card driver's own; iemmixer never applied MMCSS to it.
  Source: iemmixer `crates/iem-win/src/power.rs`, `crates/iem-audio-io/src/os.rs`; iemmixer#9 #issuecomment-5867716656 ("RT memory locked (working set +64 MiB)"); #15 body (MMCSS only for own threads).
  dante-asio: **partly** — device process goes REALTIME_PRIORITY_CLASS (crates/dante-asio/src/run.rs:199-215), no power-throttling opt-out, no `VirtualLock` of the shared mapping or pump buffers; the driver worker uses MMCSS "Pro Audio" CRITICAL with TIME_CRITICAL fallback (driver.rs:730-743). REALTIME class plus spinning priority-31 threads already starved the process's own threads on the FOH PC (run.rs:30-36 comment); iemmixer never needed REALTIME to hold 32 samples at 96 kHz.

- **Pinning the RT work to a single logical processor caused missed periods; letting the scheduler
  move it fixed them.** With the engine's CPU Set on one LP, kernel DPCs triggered by its own threads
  landed on that LP (107 ms vs 46 ms DPC per 600 s), DPC tails reached 256-512 µs (> the 333 µs
  period), and the callback had nowhere to go: 4 missed in 2.35 h, then 1 in 31 min. Without the CPU
  Set: 0 missed in 6 h 28 min and in the 8 h soak.
  Source: iemmixer#15 https://github.com/zbynekdrlik/iemmixer/issues/15#issuecomment-6038487028, #issuecomment-6039209222, #issuecomment-6043862475.
  dante-asio: **no** — the device pins the pump and RX/TX threads to one dedicated core each and spins (run.rs:22-41; dante-asio#1 notes). This is the most likely-relevant iemmixer finding for the open ~5.4 ms TX stalls (docs/HANDOFF.md "Open" 1): measure with the cores' DPC/ISR load before blaming the timer.

- **Spinning on shared cores costs the RT thread.** On the Gen 3 box, busy polling of the RX thread on
  cores shared with the pump made 3 blocks late; it was reverted and only retried with RX and the NIC
  on a core of their own.
  Source: iemmixer#72 https://github.com/zbynekdrlik/iemmixer/issues/72#issuecomment-6454902700.
  dante-asio: **partly** — the driver spins 2 000 rounds then waits on the event (driver.rs:661-683, comment cites 30 ms stalls when it only spun); the device's Windows sleep now yields (dante-asio#1). Keep spin budgets measured, per core layout.

- **Put the card's (here: the NIC's) interrupts and DPCs on a core away from the audio work; MSI must be
  on and granted first.** Tier 3 moved the card's ISR/DPC to one LP and the NIC's RSS to two others,
  NIC power saving off; card ISR max dropped from 128 to 64 µs.
  Source: iemmixer#15 https://github.com/zbynekdrlik/iemmixer/issues/15#issuecomment-6035632985; `.claude/rules/pc-tuning.md` IemTuning invariants (IRQ affinity only when MSI flagged AND granted; NIC hwid exact match).
  dante-asio: **no** — design names it (docs/design/2026-10-29-dante-asio-design.md "Realtime on Windows") but nothing applies or verifies it.

- **Measure before tuning, A/B each lever, judge by missed periods, not by DPC maxima.** 600 s runs could
  not separate Tier 1 levers (the same config gave 128 and 512 µs DPC tails); none of them changed missed
  periods. A custom power plan with EPP 0 / no core parking / USB suspend off, governor (Process Lasso)
  paused, background processes moved to housekeeping cores was kept as a reversible appliance guard;
  forcing C1 idle was rejected (no gain, heat).
  Source: iemmixer#15 https://github.com/zbynekdrlik/iemmixer/issues/15#issuecomment-6035360994, #issuecomment-5854184634.
  dante-asio: **no** tuning code; HANDOVER says Process Lasso rules were changed by hand.

- **Process Lasso matches on path too** (any exe under a `*dante*` path got the device's CPUs), and
  a process started by Task Scheduler inherits its CPUs; set your own affinity first or thread pins fail
  with error 87. MMCSS "Pro Audio" has its own `Affinity` mask that can exclude cores.
  Source: dante-asio#1 (context, already learned here); iemmixer pc-tuning (Lasso paused in tuning mode).
  dante-asio: **partly** — run.rs:22-41 sets process affinity first; nothing reads the MMCSS task's `Affinity`/`SystemResponsiveness` values and logs them at start.

- **Untuned, the IEM PC already held 32 samples at 96 kHz for 10.7 h with 0 missed**; tuning was reserve,
  not a precondition. A healthy baseline: callback p50 14 µs, interval p99.9 347 µs at a 333 µs period,
  card clock +20 ppm against QPC.
  Source: iemmixer#15 https://github.com/zbynekdrlik/iemmixer/issues/15#issuecomment-6020970215, #issuecomment-5855814254.
  dante-asio: **n/a** — useful as a target/baseline for its own histograms.

### Lifecycle, crashes, the PC

- **Every driver call on one owner thread (COM STA, pumping messages); a stream is freed only after the
  callback slot is cleared and the in-flight count is 0, waited for with a bound; a callback stuck past
  the bound parks the stream (leaked, never freed under a live callback).**
  Source: `.claude/rules/asio-spike.md` lines 20-21; iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5855814490.
  dante-asio: **no** — as a driver, the analogue is the worker that calls the DAW: `stop`, `disposeBuffers`, `createBuffers` and the last `Release` all `join()` the worker without a bound (driver.rs:155-161, 211-228, 300-306, 476, 535). A DAW whose `bufferSwitch` blocks on a lock held by the thread calling `stop` deadlocks the DAW forever.

- **Release the hardware before the process dies.** iemmixer's SEH filter asks the owner thread to release
  the driver, waits ≤ `SEH_WAIT` for `RELEASED`, then lets the process end, or parks the faulting thread if
  the card is still held; crash and WER dialogs are off so a dialog in session 1 never keeps the card held.
  Tested on the PC by an injected structured exception (exit 0xC0000005 path, engine respawned in ~5 s).
  Source: iemmixer `asio/seh.rs`, `iem-win/src/errmode.rs`; iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5914957131; #35.
  dante-asio: **no** — the device process has no SEH filter, no `SetErrorMode`/`WerSetFlags(NO_UI)`. A WER dialog on the device keeps the Dante device's ports bound and the mapping alive. (Not for the DLL: never install a process filter in the DAW.)

- **Stops are requests plus a bounded wait, then an alarm; nothing is force-killed; a reboot is graceful
  (`/t 0` without `/f`, since any `/t > 0` implies `/f`).**
  Source: `.claude/rules/guard.md` "Stops are requests", `.claude/rules/pc-tuning.md` "Nothing is force-ended (I8)".
  dante-asio: **partly** — device stops via a named stop event or Ctrl+C (run.rs:118-121, 130 `pump.join()` unbounded); no supervisor, no restart policy (dante-asio#1: "runs as a scheduled task, no supervision").

- **Supervisor exit-code policy:** 0 stays down; usage/site and card errors stay down with an alarm; any
  other exit respawns with backoff 1, 2, 4, 8, 10 s; 3 crashes in 10 min = crash loop; "state busy" retried
  without counting. A respawn checks/restores PC state first.
  Source: `.claude/rules/guard.md` "Engine exits".
  dante-asio: **no** — no exit-code contract, no respawn.

- **Task Scheduler's job object may forbid breakaway; children then die with the task unless handled.**
  First engine start failed "Access is denied (os error 5)". The guard reads its job limits and decides.
  Source: iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5865765303.
  dante-asio: **unknown** — the device runs from scheduled task `dante-asio-1`; a future tray/supervisor spawning it must check the same.

- **Session end (WM_ENDSESSION) and OS restart must release cleanly**: the engine logged "the Windows
  session ends → saving, fading out, releasing the driver"; test #1 PASS. Fast Startup hibernates the
  session (no boot trigger); it is off only when the hibernation file is absent.
  Source: iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5876275087; `.claude/rules/guard.md` boot task.
  dante-asio: **no** — no session-end handling in the device (Dante goodbyes, flow stops) or in the driver.

- **Elevated-path hygiene on the PC:** elevated sessions import scripts/run exes only from admin-only
  copies checked by SHA-256; `Add-Type` compiles with TEMP set to an admin-only folder; `PSModulePath`
  pinned; ssh starts PowerShell by literal path; OpenSSH `DefaultShell` set so HKCU AutoRun never runs.
  Source: iemmixer#15 https://github.com/zbynekdrlik/iemmixer/issues/15#issuecomment-6035882143, #issuecomment-6055677325; `.claude/rules/asio-spike.md` "stage rule".
  dante-asio: **no** — the device runs elevated from `C:\Program Files\dante-asio` (HANDOFF); install path and update path are not yet scripted (dante-asio#1 checklist).

- **Shared objects between an elevated service and a non-elevated user need an explicit DACL**; named
  pipes need `PIPE_REJECT_REMOTE_CLIENTS` and first-instance flags; `interprocess` named pipes have no
  read timeout on Windows.
  Source: iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5856211791, #issuecomment-5846897301.
  dante-asio: **yes** for shm (commit 37f19b4, crates/shm/src/windows.rs); n/a for pipes today.

- **Only a green CI bundle reaches the PC, with `SHA256SUMS` checked on the dev box and on the PC (later
  attested).** A running exe is renamed aside, never deleted (`.old-<sha>[.n]`), so reactivation never
  hits "Access is denied".
  Source: `.claude/rules/asio-spike.md` line 32; iemmixer#9 https://github.com/zbynekdrlik/iemmixer/issues/9#issuecomment-5871468450.
  dante-asio: **partly** — CI `bundle` job on windows-2025 builds the release; install is manual. A loaded DLL cannot be replaced while any DAW has it loaded: same rename-aside rule applies to `asio_driver.dll`.

### Dante / clock on Windows (from #72 and the recons)

- **dantesync is the only PTP owner; it steps the date daily** (coordinated step; on the box the engine
  held its clock ~5.6 s around it). Frequent micro steps once starved DVS's ASIO input.
  Source: iemmixer#72 https://github.com/zbynekdrlik/iemmixer/issues/72#issuecomment-6486031172; dante-asio `docs/design/recon-dantesync-windows.md`.
  dante-asio: **partly** — follows dantesync (dante-stack clock); no test of the daily step on Windows yet.

- **dantesync's time server missed 200 ms replies under heavy flow load on the same NIC** (fixed in
  dantesync 1.20.0: time server off its sync loop). Use ≥ v1.20.0 on the FOH PC.
  Source: iemmixer#72 https://github.com/zbynekdrlik/iemmixer/issues/72#issuecomment-6429797840.
  dante-asio: **unknown** — check the PC's dantesync version.

- **No workaround for a misconfigured transmitter** (the DVS "reply from another port" fix was reverted).
  Source: iemmixer#72 https://github.com/zbynekdrlik/iemmixer/issues/72#issuecomment-6522418884.
  dante-asio: **yes** (HANDOFF rules).

---

## 2. Robustness mechanisms iemmixer's ASIO host has that dante-asio lacks

| Mechanism | iemmixer | dante-asio today | Where it belongs |
|---|---|---|---|
| Fault containment of code panics | Non-allocating RT panic hook; `catch_unwind` at the backend boundary, outputs stay zero, payload never freed on RT thread (`rtpanic.rs`, `asio/backend.rs`) | None in the DLL. Rust 2024 `extern "system"` fns abort the process on unwind, so any panic (e.g. a poisoned `Mutex::lock().unwrap()`, driver.rs:135, 139, 217, 233, 259, 266, 562) kills **Ableton** | DLL: `catch_unwind` around every IASIO method and the worker body, return `ASE_HW_MALFUNCTION`; no `unwrap` on locks |
| SEH / native-crash containment | Process SEH filter: release hardware, bounded wait, then end or park (`asio/seh.rs`); dialogs off (`errmode.rs`) | None | Device process only (never the DAW's process) |
| Owner-thread model | All driver calls on one STA thread that pumps messages | Host calls arrive on any thread; state behind one `Mutex` | DLL: document and test which IASIO calls may race the worker |
| Bounded stop + parking | `finish` waits for in-flight = 0 with a bound (≈ 2 s, pumping); a stuck callback parks the stream, never freed | `stop_worker` joins without a bound (driver.rs:155-161) | DLL: bounded join; on timeout detach the worker and leak its state, report `ASE_HW_MALFUNCTION` |
| Reset budget | ≤ 1 reopen / 5 min, ≤ 3 per process, stall = 2 s without callback (`reset.rs`) | One reset per device reappearance, unlimited (driver.rs:644-649) | DLL: rate-limit `kAsioResetRequest` |
| Measured period | Period from first sample positions; anything else refuses (`period.rs`) | Not checked; `seq` gaps not counted | DLL: count `seq` jumps > 1 as DAW-missed blocks into the header |
| Logging out of the callback | Messages counted in atomics, owner thread logs every tick, each open/reopen with cause and timings | DLL logs nothing; DAW-side behaviour (outputReady use, time-info, message selectors, reopen cause) invisible; Ableton's use of `outputReady` is still an open question (recon-asio-driver.md) | DLL: atomics in the shm header (or a DLL-local ring) read and logged by the device process |
| Telemetry histograms | 1 µs-bucket histograms of callback interval and callback time, overflow = missed, capped 1 ms (`hist.rs`), polled by the guard | Device has pump counters; nothing measures the DAW's callback interval or time; TX arrival-minus-stamp missing (the 5.4 ms stalls read 0) | Both: DLL measures DAW callback time/interval; device measures send lateness |
| Stuck/dead peer detection | Stall rule (2 s) + guard health reads | Driver treats 50 ms silence as "device gone" (driver.rs:42) but never reads `device_pid`/`heartbeat_ms` (written at run.rs:99-101, shm/windows.rs:145) | DLL: use pid + heartbeat to tell "device dead" from "device late" |
| Memory locking / priority policy | HIGH class, power throttling off, working set grown, buffers `VirtualLock`ed | REALTIME class (run.rs:199-215), no throttling opt-out, no locking | Device: drop to HIGH unless a measurement proves REALTIME needed; throttling off; lock the mapping |
| Session end | `WM_ENDSESSION` → save, fade, release; never borrow the owner mid-open | None | Device: hidden window or `SetConsoleCtrlHandler` for logoff/shutdown, send Dante goodbyes |
| Supervisor | Guard with exit-code policy, backoff, crash-loop fallback, alarms | Scheduled task only | Device: tray or service supervisor |
| Stale output on reattach | Outputs zeroed before `start()` | TX halves not cleared at `createBuffers`/`start` | DLL: zero both TX halves on `createBuffers` and `start` |
| Timestamps | QPC-based (`telemetry`, ns→QPC) | `systemTime` from `timeGetTime()` in ms × 1e6 (driver.rs:710-713) — 1 ms resolution, jittery for hosts that use it | DLL: QPC-derived ns |

---

## 3. Test / CI practices to adopt

- **Hosted Windows runner as a real platform gate.** iemmixer's `windows` job runs the library tests and
  the PowerShell 5.1 self-tests against real backends on the runner; the `asio-spike` job builds the Windows
  exe, runs it with no driver (must exit 3 `no-driver`), measures trace-stop timing and produces a bundle
  with `SHA256SUMS`. dante-asio has `test` on windows + `bundle`; add: an in-CI ASIO host test that loads the
  built DLL via COM with a stand-in device (host.rs exists — extend it to stop-while-in-callback, reset
  storm, device death/restart, panic injection) and an ASIOTest-style conformance run if a CLI exists.
- **Mutation testing, diff-scoped and hard-bounded.** Decisions live in portable, mutated modules
  (`owner.rs`, `reset.rs`, `period.rs`, `messages.rs`, `telemetry.rs`, `prefwin.rs`); FFI files
  (`asio.rs`, `os.rs`) are excluded. Nextest `mutants` profile: fail-fast, `terminate = "immediate"`,
  `slow-timeout` 2 × 5 s so hung mutants end. Rules: avoid `>`/`>=` where both agree, pack bit-fields with
  `+` not `|`, bounded test waits (5 s). dante-asio has no mutation job: move driver decisions (reset
  budget, quiet/dead verdict, seq-gap accounting, late accounting in `bridge.rs`) into pure modules and gate them.
- **RT proofs:** `assert_no_alloc` test binary, rtsan on a pinned nightly (`-Zsanitizer=realtime`),
  bench with p50 ≤ 25 % period / worst p50 ≤ period. Apply to `Bridge::process` and the driver worker's
  per-block function (made portable).
- **Coverage floor** (`cargo llvm-cov --fail-under-lines`): none in dante-asio.
- **Integrity scan in CI:** bans forbidden driver calls and every force-kill / forced-restart form across
  code, scripts and workflows (`scripts/check_integrity.py`). dante-asio could ban `REALTIME_PRIORITY_CLASS`
  outside one audited spot, `unwrap()` in `asio-driver`, and opening UDP 319/320.
- **Supply chain:** engine dependency allowlist with a reason per crate, cargo-deny plus osv-scanner over
  every lockfile.
- **PC windows and HIL:** only a green CI bundle reaches the PC; HIL v1 runs from the verified bundle
  (11 checks: build/version, card at the block with 0 missed/0 resets for 120 s, private pipes, test signal
  only on spare outputs, forced reopen, panic → respawn, alarm push), later through a self-hosted runner that
  posts a required check on the commit. Fault injections (`inject-fault`, `inject-seh`, `inject-park`) are
  dev-only, behind `--fault-injection` and a HIL job. Reliability tests on the real PC: OS restart with
  the engine running, restart with a parked engine, one hard kill, SEH injection, round-trip loop latency
  through Dante (spare TX → own spare RX).
- **Soak gate:** ≥ 8 h, 0 missed periods, late intervals ≤ 0.2 %, process p99.9 under a limit, stream frames
  with 0 holes; counters polled every minute so each event gets a time. dante-asio's MVP asks "1 h clean";
  iemmixer's evidence shows 600 s cannot separate levers and multi-hour runs found the single-LP pinning
  failure.
- **Measure with an external receiver**, not only own counters (dante-stack proposal item 4): iemmixer's
  HIL read engine histograms; the dante-asio 5.4 ms stall shows own counters can read 0.
- **Reject-case tests not shadowed:** each negative test supplies every mandatory input and fails only on
  the one flag under test.

---

## 4. Open questions only iemmixer's session or the owner can answer

1. Did iemmixer ever see Ableton (or any host) call `outputReady` / `bufferSwitchTimeInfo`, and does Live
   re-read `getLatencies` after `kAsioResetRequest`? (iemmixer was the host, so likely not — owner/test.)
2. Is the FOH PC's dantesync ≥ 1.20.0 (the release that fixed missed time-server replies under load)?
3. iemmixer kept the engine at HIGH_PRIORITY_CLASS. Is REALTIME_PRIORITY_CLASS for the device process an
   owner decision, or may dante-asio drop to HIGH and measure?
4. The owner dedicated two cores to dante-asio via Process Lasso. Given iemmixer's single-LP result
   (#15), may dante-asio un-pin its threads (keep the process on its cores, let threads float) as an A/B
   for the TX stall?
5. Should dante-asio get the same §10-style reliability set (OS restart with the device running, hard kill
   of the device while Ableton plays, DAW crash while the device plays) run on the FOH PC in dev time, and
   who signals dev time on that PC (iemmixer uses EVENT-NOW / the owner's "ide event")?
6. Does the FOH PC have Fast Startup active (hiberfil present)? It decides whether boot-time tasks run.
7. Was the Windows-side tuning tooling (`IemTuning.psm1` power plan / IRQ affinity / journal with undo) meant
   to move to a shared place (dante-stack or fohmixer) rather than be re-written here? The FOH PC is shared
   with fohmixer, which owns CPU pinning per the peer channel.
