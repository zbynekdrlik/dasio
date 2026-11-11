# dasio

Repo `zbynekdrlik/dasio` (named `dante-asio` before 31.10.2026). Everything is named dasio (owner, 3.11.2026): crate and binary `dasio`, driver `dasio_driver.dll`, service `dasio-<n>` running `dasio-<device name>.exe` (one exe per instance, so Process Lasso can pin each), `C:\ProgramData\dasio`.

A Windows ASIO driver whose audio goes straight to our own Dante device (one device process per instance).
Design: `docs/design/2026-10-29-dante-asio-design.md`; MVP plan: `docs/plans/2026-10-29-dante-asio-mvp-plan.md`.

- Branches: `dev` and `main` only; PRs dev → main; merge commits.
- No local cargo: GitHub CI (`ubuntu-latest`, `windows-latest`) builds and tests.
- Version: `[workspace.package].version`; dev > main on PRs (`scripts/check_version.py`).
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
