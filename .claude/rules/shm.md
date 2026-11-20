---
paths:
  - "crates/shm/**"
  - "crates/dasio/src/run/serve.rs"
---

# The shared memory (crate `shm`)

- The device locks its views in its own memory (dasio#28 E11): `Base::serve` calls `Shared::lock` right after `Shared::create`, for the first serving and every re-serve. Only the section and the directory views are locked: dstack's buffers, the threads' stacks and the code stay pageable, and the DAW's own view of the section can still be trimmed from the DAW's working set. So a lock removes page faults on these views only, never every fault on the pump. A new device-side view goes into `Shared::views()` so it is locked and counted too. The driver side (`Shared::open`, in the DAW's process, and `dasio probe`) never locks: never change the DAW's process.
- `lock` raises the working set first, then `VirtualLock`s; a failure is a WARN with its step and the device serves unlocked. The limits (soft) are `lock.rs`'s `working_set`, both raised by all locked bytes (the `LOCKS` mutex, across sections) and `LOCK_MARGIN`: the minimum (the lock quota, guaranteed resident) on top of the process's own minimum before its first lock only, the maximum on top of the larger of its own maximum and its use now (`GetProcessMemoryInfo` working set and peak). Never set a maximum below the use: the device holds tens of MB against Windows' ~1.4 MB default maximum, and a lower maximum trims its unlocked pages, which the pump then faults back. The INFO line carries `page_faults_before`/`after`: the lock faults its own pages in (up to bytes / 4096), a jump well past that is the raise trimming.
- The arithmetic is portable in `lock.rs` (mutated); the calls stay in `windows.rs` (excluded from mutants).
- Windows tests check a lock by QueryWorkingSetEx per page: bit 0 Valid, bit 22 Locked, per virtual address (a second view of the same section in the same process reads unlocked). `cargo test` runs a binary's tests in parallel threads of one process, so in `tests/windows.rs` never assert the process-wide locked total or an exact working set (assert lower bounds; read the use BEFORE the lock). An exact count goes in a test binary of its own: `tests/windows_lock_count.rs` (`locked_bytes()`).
- Instance numbers: in `tests/windows.rs` 77, 89-92, 94-98 are taken, and 93 and 99 must stay unused (93: the not-found test; 99: `another_rate_at_the_same_block_makes_a_fresh_section` asks a rate of it and expects NotFound). 88 belongs to `tests/windows_lock_count.rs`. 1-16 are taken by the driver's host and conformance tests, the e2e test (13, 15) and the service test (14).
