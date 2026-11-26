---
paths:
  - "crates/dasio/src/rx_arrival.rs"
  - "crates/dasio/src/rx_arrival/**"
  - "crates/dasio/src/app_status.rs"
  - "crates/dasio/src/run.rs"
  - "crates/dasio/src/run_bridge.rs"
---

# The measured RX arrival (dasio#128)

- Owner ruling 16.11.2026: a Dante device's latency is NEVER set automatically, at runtime or at restart, and dasio never computes or proposes a value. Every latency change is a deliberate, owner-approved change decided from dcontroller's long-term per-device graph. So no "recommended latency_ns", no margin or rounding to a step, no WARN "configured below", no icon state from it. (0.1.111 shipped such a recommendation for a few hours; 0.1.112 replaced it.)
- What dasio shows is plain measurement: the RX packets' age at arrival (our PTP at the packet's arrival less its stamp: transit and our clock's offset; since dstack v0.6.157 the writer's wait to the read is left out, on Windows bracketed by the reads), per device over the slots that received: the peak since start (dstack `RxCounts::lateness_peak`, samples at the DEVICE rate) and the least..largest of the heartbeat's last second (`RxShared::last_min_lateness_ns` / `last_lateness`) as the 10 s look found it, so up to ~10 s old; the pair sits under one lock (no torn least/largest). An idle slot's 0 is no measurement; `Arrival` keeps the peak since start so a slot dstack re-plans never lowers it.
- Never call `take_max_lateness`: dstack's heartbeat owns it, a second reader would steal its second.
- The clock sign (dasio#237, owner 25.11.2026: one look at what keeps a PC from low latency). An RX arrival mean
  under `app_status::CLOCK_SIGN_MEAN_NS` (0.05 ms) is one item of `app_status::readiness`, beside the PC profile and
  the TX path. FOH band read 0.18 ms and mbc 0.19 ms; alex-nb read 0.02 ms (0.00 before its silent RX flow, dasio#234).
  It is text and a JSON key only (`low_latency`: `ok` / `not` / `checking`). Only a failing item shows in the tray
  line, as ` · low latency NOT: ...` before the RX arrival part (bridge: before its underruns). It never sets the
  icon and never proposes a latency.
  - `ok` and `checking` are silent in the line by choice, so a ready PC's line stays short. The issue's first design
    had a ` · low latency OK` text.
  - `checking` can come back on a 10 s tick that placed no packet, since the mean is per tick (`None` without one).
    Read it next to `rx_arrival_mean_ns`, never as a fault.
  - A failing item repeats the PC profile's or TX's own figure on purpose: the issue asks for each item with its
    number in one look.
  - The bridge object carries `low_latency` right after `state` (startup-counts.md).
  - The TX age item (dstack #500, v0.6.224): `run/serve.rs` reads `media.tx.tx_age_view()` in the status call, never
    on a media thread, since it locks the TX table. `app_status::worst_tx_age` keeps the lane furthest past its
    receiver's latency (one receiver, a known latency and a measured 10 s window only). `TX age X ms over the Y ms
    of <receiver>` shows when its age is past the latency (`>`, as dstack's `over_latency`). Never call
    `take_age_peak`: dstack's late watch takes it once a second. The bridge kind passes no TX age yet.
- The MEAN age (dstack v0.6.156 running totals, arrival age since v0.6.157 / dstack issue 350): `Flow` carries each slot's `lateness_sum_ns`/`lateness_count` and `read_wait_sum_ns`/`read_wait_count`; `Watch` keeps its OWN previous look per slot and `measure_since` diffs them each counts tick: the device mean = Σ sum deltas / Σ count deltas (per packet, not a mean of slot means), sums diffed modulo 2^64, a slot whose count went DOWN restarted (its new totals count from zero), a tick without a new packet has no mean (`None`, never 0), the first look counts from the device's start. Never fake a mean from maxima; never use dstack's `lateness_mean_ns_since` across a restart (its modulo count diff gives garbage).
- The read wait (dstack v0.6.157): the writer's wait from arrival to its read, reported apart (`read_wait_*`): its tick mean and its peak since start (`Arrival` keeps the largest seen). At a bridge it is the ASIO driver's buffer switch pull, so only `bridge_json` names it: `rx_asio_pull_wait_mean_ns` (tick with a packet) and `rx_asio_pull_wait_peak_us`. A DAW instance's status never labels its wait an ASIO pull (its writer is the RX thread or the pump); its log line carries it as `read_wait_*`.
- Where: the run loops (`run.rs`, `run_bridge.rs`) call `Watch::observe` once a counts tick (10 s) and log INFO when the peak since start rose; the PC profile check (10 min) logs the figures again. The status closures read the shared `Arrival`.
- Status JSON keys `rx_arrival_peak_ns`, `rx_arrival_last_min_ns`, `rx_arrival_last_max_ns`, `rx_arrival_mean_ns` appear only once measured (the last two only for a second with a packet, the mean only for a tick with one), never a 0 that reads as a measured age. dtray's line: ` · RX arrival 0.10-0.35 ms, mean 0.21 ms, peak 0.72 ms` (without the last second: ` · RX arrival mean 0.21 ms, peak 0.72 ms`).
- Name the method `observe`, not `look`: `app_status/tests.rs` guards `run_bridge.rs` against any `.look(` call (dstack 0.6.151's removed age look).
- dstack judges a packet late against its FLOW's playout (`RxPlan.latency`, at least the device latency), so a peak above `latency_ns` is not by itself a late packet: read it next to the late counts.
- A bridge instance pulls its RX at the ASIO driver's buffer switch (dstack v0.6.155, dasio issue 120). Up to dstack v0.6.156 the age at placement included up to one driver buffer period of that wait; since v0.6.157 (dstack issue 350) the age is the arrival's again (on Windows bracketed by the reads: an on-time packet reads at most one packet interval high, a late burst just before a read shows in dstack's `lateness_at_read_peak`, not `lateness_peak`) and the wait is the separate ASIO pull wait.
- dstack's `read_wait_peak_us` is in µs while the sums are ns: a slot can count waits with a 0 µs peak, so a wait is measured when it is counted OR has a peak (`measure_since`); the mutation gate needs a test for each side of that `||` alone (PR 160).
- Reading a device status `rx` entry by hand: `latency`, `playout`, `delay`,
  `late_after`, `advertised`, `lateness`, `lateness_peak` are SAMPLES at the
  device rate, not us (dsg 18.11.2026: `latency 803, late_after 864` at
  96 kHz = 8.36 / 9.0 ms; misread as 0.8 ms it blamed songplayer-pp's sender
  for packets the network had delayed 6-9 ms, dstack #424/#425). The `_ns`
  and `_us` keys are what they say.
