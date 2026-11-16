---
paths:
  - "crates/dasio/src/app_rate.rs"
  - "crates/dasio/src/app_rate/**"
  - "crates/dasio/src/run/serve.rs"
  - "crates/asio-driver/src/rate.rs"
  - "crates/shm/src/**"
---

# An ASIO app at its own sample rate (dasio#107)

- The Dante rate is read-only (owner, 13.11.2026): the device stays at 96 kHz; an app at 44.1 or 48 kHz (`shm::APP_RATES`) is converted in the device process, never in the driver.
- The path: driver `setSampleRate` -> `requested_rate` in `Global\dasio-<n>-dir` -> the device's 100 ms loop -> `run::serve::Base::serve(Plan)` re-serves in process (new section, new pump; the Dante device runs on). The directory outlives sessions, so a device restarted under a DAW comes back at the DAW's rate.
- The processor: `Bridge` alone at 96 kHz (bit-identical to before); `dlink::rated::Rated<Reblock<Bridge>>` otherwise. ASIO needs a fixed buffer: `Reblock` gives the bridge `ceil(B * R / 96000)` frames. 48 kHz on an even block is exact (half-band, one DAW block per device block, no prime); 44.1 kHz needs a TX prime of one DAW block.
- The bridge runs in app samples: its block starts, its deadline clock (`to_samples(ns, app_rate)`) and its offset (`Plan::wait_offset`, rounded down) are all at the app's rate. Its wait counts from the DEVICE block's start (`bridge::WaitFrom`, set by the reblock before it runs that device block's DAW blocks), not the DAW block's: at 44.1 kHz a DAW block starts up to a block earlier. `Served::new` takes the bridge's `wait_from()`.
- A change to the directory or header layout bumps `shm::VERSION` and `install.ps1`'s `$ShmProtocol` together (shm's test checks they agree); `Shape::new` keeps the pump's blocks, `Shape::app` the app's.
- Reported latencies: `Plan::latencies` = the device's terms at R + `dlink::rated::delay_ns` + the reblock's share (A/2 in, A - A/2 out on 44.1, none on 48). The pulse test in `app_rate/tests.rs` measures the real path against it.
- Tests run the converter as it ships: `Cargo.toml`'s `[profile.dev.package.*]` overrides build rubato, dstack-resample, dlink and the audioadapter crates with `opt-level = 3` AND `debug-assertions = false`, `overflow-checks = false`. Opt-level alone is not enough: debug assertions turn on std's unsafe-precondition checks inside rubato's AVX loop (111-183 us a block at the e2e shape; the coverage job's 44.1 kHz step overran). A new crate on the converter's hot path gets the same three lines. `bench_bridge.rs` holds the conversion's own cost at the e2e shape under a quarter of the device block.
