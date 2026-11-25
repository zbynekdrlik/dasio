# dasio

Repo `zbynekdrlik/dasio` (named `dante-asio` before 31.10.2026). Everything is named dasio (owner, 3.11.2026): crate and binary `dasio`, driver `dasio_driver.dll`, service `dasio-<n>` running `dasio-<device name>.exe` (one exe per instance, so Process Lasso can pin each), `C:\ProgramData\dasio`.

A Windows ASIO driver whose audio goes straight to our own Dante device (one device process per instance).
Design: `docs/design/2026-10-29-dante-asio-design.md`; MVP plan: `docs/plans/2026-10-29-dante-asio-mvp-plan.md`.

- Branches: `dev` and `main` only; PRs dev → main; merge commits.
- No local cargo: GitHub CI (`ubuntu-latest`, `windows-latest`) builds and tests.
- CI runs only on pushes to main/dev and PRs into them. A worker branch gets its CI by being merged into dev, never through a draft PR into dev: that PR's version check fails at dev's own version and stays on the head SHA as a red check, leaving the dev→main PR UNSTABLE (PR #217).
- Version: `[workspace.package].version`; dev > main on PRs (`scripts/check_version.py`). Cargo.lock holds it three times (asio-driver, dasio, shm): bump all three, or CI's `cargo deny --locked` fails.
- No site data in the repo: addresses, real device and channel names live only in the PC's config.
- Dante: never change another device's subscriptions; tests subscribe our own device to itself.
- Dante code comes from dstack (zbynekdrlik/dstack#1): `dstack` and `dstack-audio-io` are git deps pinned to a tag in `crates/dasio/Cargo.toml`. A Dante protocol bug goes there as a ticket; this repo only moves the tag.
- dantesync is the PC's only clock owner: it alone sets the system clock. The device follows Dante PTP in process by default (dstack's `ClockSource::Linked`, read only, needs Npcap; owner 4.11.2026), with the dsync service as its fallback; `--clock dsync` uses the service only.
- Every message to the owner in Slovak; code, commits and docs in English.

## Playbook router
- switching a DAW onto an instance → `.claude/rules/daw-device-switch.md` (auto-loads on its `paths:`)
- the NDIS filter, kernel TX, reinstalling it → `.claude/rules/ndis-pacer.md` (auto-loads on its `paths:`)
- the PC profile (FOH Windows settings, Lasso) → `.claude/rules/pc-profile.md` (auto-loads on its `paths:`)
- dstack deps, the device's address and port slot → `.claude/rules/dstack-deps.md` (auto-loads on its `paths:`)
- the driver's host tests (packed ABI fields, instance numbers) → `.claude/rules/asio-driver-tests.md` (auto-loads on its `paths:`)
- the bridge instance kind (`[bridge]`, a local ASIO driver) → `.claude/rules/bridge-kind.md` (auto-loads on its `paths:`)
- an ASIO app at its own sample rate (conversion, re-serve, latencies) → `.claude/rules/app-rate.md` (auto-loads on its `paths:`)
- the measured RX arrival (information only, never a latency proposal) → `.claude/rules/rx-arrival.md` (auto-loads on its `paths:`)
- the shared memory, its locked views → `.claude/rules/shm.md` (auto-loads on its `paths:`)
- the output watch, its test-tone check (`watch.toml`, its log lines) → `.claude/rules/miss-watch.md` (auto-loads on its `paths:`)
- the kernel trace of an audio thread (`scripts/trace/`, capture, cut, readers) → `.claude/rules/kernel-trace.md` (auto-loads on its `paths:`)
- the CI mutation gate (Windows glue, bounded loops, equivalent mutants, shards) → `.claude/rules/mutation-gate.md` (auto-loads on its `paths:`)
- the boot wait for a calm Windows, the start-up counts kept apart (`phase=`) → `.claude/rules/startup-counts.md` (auto-loads on its `paths:`)
