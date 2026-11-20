---
paths:
  - "crates/shm/**"
  - "crates/dasio/src/run/serve.rs"
---

# The shared memory (crate `shm`)

- The device locks its views in memory (dasio#28 E11): `Base::serve` calls `Shared::lock` right after `Shared::create`, for the first serving and every re-serve. A new device-side view (another mapping the pump or bridge touches) goes into `Shared::views()` so it is locked and counted too. The driver side (`Shared::open`, in the DAW's process, and `dasio probe`) never locks: never change the DAW's process.
- `lock` raises the working set first (soft limits, the process's own base + all locked bytes + `LOCK_MARGIN`, kept in the `LOCKS` mutex across sections), then `VirtualLock`s; a failure is a WARN with its step and the device serves unlocked. The arithmetic is portable in `lock.rs` (mutated); the calls stay in `windows.rs` (excluded from mutants).
- Windows tests check a lock by QueryWorkingSetEx per page: bit 0 Valid, bit 22 Locked, per virtual address (a second view of the same section in the same process reads unlocked). `cargo test` runs the tests in parallel threads of one process, so never assert the process-wide locked total or an exact working set; assert lower bounds.
- Instance numbers in `crates/shm/tests/windows.rs`: 77, 90-92, 94-99 are taken; 93 must stay unused (the not-found test).
