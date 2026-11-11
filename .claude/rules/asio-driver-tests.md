---
paths:
  - "crates/asio-driver/tests/**"
  - "crates/dasio/tests/e2e.rs"
---

# Driver host tests

- The host tests build only on Windows (`#![cfg(windows)]`) and there is no local cargo: a compile error shows up only in CI. Read these two traps before pushing:
  - The ABI structs in `src/abi.rs` are `repr(C, packed(4))`. A reference to a field aligned above 4 is error E0793, and `assert_eq!`, `assert_ne!`, `format!` and `.iter()` all take one. `ASIOBufferInfo::buffers` (pointers) is such a field: copy it first (`let [a, b] = info.buffers;`).
  - Most ABI structs derive nothing. `[S { .. }; N]` needs `Copy`; use `core::array::from_fn`.
- Each test owns its instance number (1 to 16; shared memory and events are named `Global\dasio-<n>-*`, so two tests on one number collide across test binaries). Taken: 3 conformance, 4-12 and 16 host tests, 13 and 15 the e2e test, 14 the service test. Update the list in `crates/dasio/tests/e2e.rs` when you take one.
- `tests/common/mod.rs` is compiled into every test binary that declares `mod common;`. Each such binary must use all of its pub functions, or `dead_code` fails clippy `-D warnings`.
- To measure the driver's worker thread (its CPU, say), record `GetCurrentThreadId()` in the test's bufferSwitch callback: it runs on the worker. `Rt::worker_tid` is private. Use `GetThreadTimes` on that tid, not `GetProcessTimes`: the other tests in the binary run in parallel and add noise to the process figure (dasio#87).
- windows-sys gates some Threading functions on other features. For example, `CreateWaitableTimerExW` needs `Win32_Security`. Check `#[cfg(feature = ...)]` in the registry source before you push.
