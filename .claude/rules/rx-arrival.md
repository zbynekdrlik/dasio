---
paths:
  - "crates/dasio/src/rx_latency.rs"
  - "crates/dasio/src/rx_latency/**"
  - "crates/dasio/src/app_status.rs"
  - "crates/dasio/src/run.rs"
  - "crates/dasio/src/run_bridge.rs"
---

# The recommended RX latency (dasio#128)

- dasio only REPORTS the receiver latency the measured arrival asks for; it never changes `latency_ns` while it runs. The session (or the owner) sets it in device.toml at the next restart (owner ROZHODNUTÉ 16.11.2026). Do not add a runtime change: `latency_ns` is also what our TX advertises, a change re-sets every subscription's playout, and a device that tunes itself down learns its peak from clicks.
- The figure: dstack's `RxShared::lane(slot).lateness_peak` (samples at the DEVICE rate, the largest packet age at placement since the device started) over the slots with `packets > 0`; an idle slot's 0 is no measurement. `rx_latency::Peak` keeps dasio's own max since start, so a slot dstack re-plans never lowers it. Recommended = peak + 0.25 ms margin, rounded up to 0.25 ms. Peak since start, never a window (dsg 15.11: bursts every ~96 s a short window misses).
- Where: the run loops (`run.rs`, `run_bridge.rs`) feed `Watch::observe` once a counts tick (10 s) and log only when the recommendation changes; the PC profile check (10 min) logs the current report again. INFO when it holds, WARN when the configured `latency_ns` is below it. The status closures read the shared `Peak`.
- Status JSON keys (`rx_latency_ns`, `rx_arrival_peak_ns`, `rx_latency_recommended_ns`, `rx_latency_below`) appear only once measured, never a 0 that reads as a measured age. dtray's line: ` · RX 1.00 ms (rec. 0.75)` or ` · RX 1.00 ms < rec. 1.25 ms`; below = icon `warn`.
- Name the method `observe`, not `look`: `app_status/tests.rs` guards `run_bridge.rs` against any `.look(` call (dstack 0.6.151's removed age look).
- Known limit: dstack judges a packet late against its FLOW's playout (`RxPlan.latency` = P_flow, at least the device latency P), and v0.6.151 exposes no per-slot plan. A flow read later than the device latency (P_flow > P) can lift the peak above `latency_ns` with no late packet, so the report then over-recommends and WARNs. Subtracting each slot's `P_flow - P` needs a dstack accessor (a dstack ticket), never a guess in dasio.
- `flows` is tested against a real `RxShared` on the loopback (the mutation gate runs `--in-diff`).
