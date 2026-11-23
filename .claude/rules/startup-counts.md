---
paths:
  - "crates/dasio/src/startup.rs"
  - "crates/dasio/src/startup/**"
  - "crates/dasio/src/boot_calm.rs"
  - "crates/dasio/src/boot_calm/**"
  - "crates/dasio/src/service/windows.rs"
  - "crates/dasio/src/run.rs"
  - "crates/dasio/src/run/serve.rs"
  - "crates/dasio/src/run_bridge.rs"
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

## The start-up counts (`startup::Split`)

- The first 5 min after the device serves are its start-up window, on every
  start.
- The split counters in both counts lines are the PHASE's counts, named in
  `phase="start-up"|"running"`.
- The window's end logs the start-up counts once: `the start-up window
  ended: ...`.
- A total since the start is that line's value plus the running value.
  Compare two counts lines only within one phase.
- NOT split: peaks (`late_max_us`, `gap_max_ms`, `max_process_us`);
  progress counts (`blocks`, `callbacks`, `to_asio_pulls`, `seq`/`ack`,
  `latency_reads`); gauges; the `from_asio_pull` view; dstack's own status
  and dcontroller's figures; the per-minute CSV (already per-minute
  deltas).
- A counter that went down restarted: a new driver run (the runner's counts
  are per run), or a new section after a rate change. Its whole value is
  new, as in `LateWindow`. A restart whose new count passes the old one
  within a tick (10 s) is undercounted.
- A new split counter:
  - goes into its totals array and the destructuring after `shown` in the
    same order, in BOTH the counts tick and the `startup_ended` log;
  - if the status shows it, also into the status closure (`serve.rs` /
    run_bridge's `facts`) and `app_status::DAW_STARTUP` / `BRIDGE_STARTUP`.
- `app_status/tests.rs` guards the bridge counts line as text:
  `late_callbacks = late_callbacks,`, then `late_bins = ?late_bins,` right
  before `late_max_us = s.late_max_us,`.
