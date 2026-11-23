---
paths:
  - "crates/dasio/src/startup.rs"
  - "crates/dasio/src/startup/**"
  - "crates/dasio/src/boot_calm.rs"
  - "crates/dasio/src/boot_calm/**"
  - "crates/dasio/src/service/windows.rs"
  - "crates/dasio/src/run.rs"
  - "crates/dasio/src/run/counts.rs"
  - "crates/dasio/src/run/serve.rs"
  - "crates/dasio/src/run_bridge.rs"
  - "crates/dasio/src/run_bridge/**"
  - "crates/dasio/src/app_status.rs"
  - "scripts/ci-service-test.ps1"
---

# The boot wait and the start-up counts (dasio#28, 0.1.158)

Owner 23.11.2026: dropouts while Windows starts do not count, and dasio
starts its media only once Windows is calm after a reboot.

## The boot wait (`boot_calm`)

- Only `dasio service` waits, and only when the system uptime is under
  10 min at the service's start. `dasio run`, a restart outside that window
  and the test clock (`--clock system`) start at once.
- Why the test clock never waits: a fresh CI runner can be minutes past its
  boot. `ci-service-test.ps1` checks the line with `reason="test-clock"`.
- One sample a second, taken before the address wait:
  - the CPU busy: `GetSystemTimes` deltas, total;
  - the disk queue: PDH `\PhysicalDisk(_Total)\Avg. Disk Queue Length`.
- 20 calm seconds in a row start the media: CPU <= 20 % and queue <= 0.5,
  or the queue unread. The cap of 5 min starts it anyway, with a WARN.
- The thresholds are constants in `boot_calm.rs`. Never make them a
  device.toml key: dtray's `Fields::read` would refuse the file
  (bridge-kind.md).
- Read a boot in the instance log:
  - `a boot: the device waits for a calm Windows ...`;
  - `waiting for a calm Windows` every 10 s (`cpu_percent`, `disk_queue`,
    `calm_s`): these are each PC's real numbers for tuning;
  - the end line, `reason=calm|cap|not-a-boot|test-clock`.
- During the wait the device does not run, so dtray shows it as not
  running.
- A second instance on one PC sees the first one's spinning media threads
  in the CPU busy. It may wait for the cap.
- `boot_calm/tests.rs` guards the service's wiring as text: the wait with
  the uptime, the test clock and the reader, then its stop, then the
  address wait and the device.

## The start-up counts (`startup::Split`)

- The first 5 min after the device serves are its start-up window, on every
  start.
- In the DAW kind, a DAW that attaches opens a new window from the last
  tick (`Split::restart_window`). The owner's 21.11.2026 rule: misses count
  only from some minutes after a DAW's start.
  - `startup::Attach` decides an attach: a DAW off at the last tick, or
    one on another section (a rate change re-serves).
  - A stop and start on the same section within one tick (10 s, e.g. an
    engine toggle) is not seen.
  - The log line is `a DAW attached: its start's counts are start-up
    counts for a new window`.
  - The start-up counts sum every window's.
- The split counters in both counts lines are the PHASE's counts, named in
  `phase="start-up"|"running"`.
- Each window's end logs the start-up counts so far: `the start-up window
  ended: ...`.
- A total since the start is that line's value plus the running value.
  Compare two counts lines only within one phase.
- The tray line's `late N/min (M total)`: M is the phase's count, so after
  the window it is the running total. The ` · at start-up: ...` part beside
  it holds the rest.
- NOT split: peaks (`late_max_us`, `gap_max_ms`, `max_process_us`);
  progress counts (`blocks`, `callbacks`, `to_asio_pulls`, `seq`/`ack`,
  `latency_reads`); gauges; the `from_asio_pull` view; dstack's own status
  and dcontroller's figures; the per-minute CSV (already per-minute
  deltas).
- A counter that went down restarted: a new driver run (the runner's counts
  are per run), or a new section after a rate change. This is the same
  restart `LateWindow` detects, but here the whole new value counts.
  A restart whose new count passes the old one within a tick (10 s) is
  undercounted.
- The split's glue lives in `run/counts.rs` and `run_bridge/counts.rs`.
  Both are excluded from mutants and guarded as text in
  `startup/tests.rs`.
  - DAW kind: the whole counts line.
  - Bridge: the totals and the named `Shown`. The "bridge counts" line
    itself stays in `run_bridge.rs`: its own text guards need it there.
- A new split counter goes into its totals array and the destructuring
  after `shown`, in the same order, in BOTH the tick and its
  `startup_ended` log.
- If the status shows the counter, it also goes into the status read
  (`serve::header_totals`, which the tick and the status share, or
  `StatusSplit::shown`) and into `app_status::DAW_STARTUP` /
  `BRIDGE_STARTUP`.
- The status JSON's start-up keys come LAST (`,"phase":...`). Older tests
  match runs of keys between `pc_profile_problems` and `kernel_tx`, so a
  key inserted there splits them.
- Text guards on the bridge counts line:
  - `app_status/tests.rs`: `late_callbacks = c.late_callbacks,`, then
    `late_bins = ?c.late_bins,` right before `late_max_us = s.late_max_us,`;
  - `app_status/tests/bridge_figures.rs`: `late_bins = ?c.late_bins,`
    inside the tick.
