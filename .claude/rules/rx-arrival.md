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
- What dasio shows is plain measurement: the RX packets' age at placement (our PTP now less the packet's stamp: transit, the RX pass's wake and our clock's offset), per device over the slots that received: the peak since start (dstack `RxCounts::lateness_peak`, samples at the DEVICE rate) and the least..largest of the heartbeat's last second (`RxShared::last_min_lateness_ns` / `last_lateness`) as the 10 s look found it, so up to ~10 s old; the pair sits under one lock (no torn least/largest). An idle slot's 0 is no measurement; `Arrival` keeps the peak since start so a slot dstack re-plans never lowers it.
- Never call `take_max_lateness`: dstack's heartbeat owns it, a second reader would steal its second.
- dstack v0.6.151 exposes no MEAN age (no per-slot sum + count). A mean needs a dstack accessor; never fake one from sampled maxima.
- Where: the run loops (`run.rs`, `run_bridge.rs`) call `Watch::observe` once a counts tick (10 s) and log INFO when the peak since start rose; the PC profile check (10 min) logs the figures again. The status closures read the shared `Arrival`.
- Status JSON keys `rx_arrival_peak_ns`, `rx_arrival_last_min_ns`, `rx_arrival_last_max_ns` appear only once measured (the last two only for a second with a packet), never a 0 that reads as a measured age. dtray's line: ` · RX arrival 0.10-0.35 ms, peak 0.72 ms`.
- Name the method `observe`, not `look`: `app_status/tests.rs` guards `run_bridge.rs` against any `.look(` call (dstack 0.6.151's removed age look).
- dstack judges a packet late against its FLOW's playout (`RxPlan.latency`, at least the device latency), so a peak above `latency_ns` is not by itself a late packet: read it next to the late counts.
