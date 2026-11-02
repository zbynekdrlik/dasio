# Handover: dante-asio (2026-10-30)

Read `docs/HANDOFF.md` too: it holds the full state and the gotchas. This file adds what the go-live requires. The repository was started from an iemmixer session; the project now lives in its own account (airuleset#1270).

## Status

MVP proven by the owner's listening test on 2026-10-30: master Ableton on the driver at 64 samples, stage rack on our TX, "hra to dobre a akusticky je to bez problemov". Production was switched back: master on VB-Matrix, every receiver on the console card. Device process stays on the FOH PC as task `dante-asio-1`. Nothing here is in production yet.

The Dante code is the `dstack` and `dstack-audio-io` crates from dstack, pinned to a tag. Dante protocol bugs go to dstack; this project only moves the version.

## Machines

- FOH PC: MCP `win-ableton-foh`, the target of all of this.
- SNV Resolume PC with DVS: MCP `win-resolume-snv`, used for Dante receive checks.
- dev1: this account. No local cargo: GitHub CI builds and tests.
- Addresses: `~/.config/dasio/SITE.md`.

## What else acts on the machines

- dantesync is the PC's only clock owner; the device never opens UDP 319/320.
- Ableton master and band, VB-Matrix and the console's Yamaha card carry production audio on the FOH PC. fohmixer runs on the same PC.
- CPUs 14-15 are dedicated to this work by owner decision (issue #1); Process Lasso, MMCSS and pinning findings are there.
- Master runs as another Windows user, non-elevated; the device runs elevated, so shared objects need the explicit DACL.
- Windows updates or a replugged USB NIC can leave DVS stamping 61 ms late while PTP looks locked: restart `dvs.manager`, check latency after every DVS start.

## How to run a test

Tests subscribe our own device to itself. For a device check read the receiver's Latency tab, not only routing status. Before any switch of the master Ableton: compare Repro L/R against Repro 0db with VB-Matrix's XML, because a swap plays the speakers at full level. Use File > Save a Copy, never overwrite master.als without the owner. Restart Live after installing the driver and enable all channel pairs.

## Site facts

Private, on this account: `~/.config/dasio/SITE.md`, `ablfoh-1.toml`, `master.toml`, `tools/`.

## Runners

None needed for now: CI runs on GitHub-hosted Linux and Windows. The real-hardware gate lives in dstack.

## Backlog

Owner-requested (29.10.):
- Windows Dante ASIO bridge in Rust with a tray icon for basic settings, such as the channel count.
- Several instances, each with its own buffer: `master-asio-dante` and `band-asio-dante`.
- Low-level realtime NIC access like dantesync; verify the NIC driver type.
- Patchable and renamable in Dante Controller.
- Both Abletons directly on Dante at the lowest latency, replacing VB-Matrix.

Open from the MVP, in order: `docs/HANDOFF.md` "Open" list 1-6. Items 1-3 and 5 are Dante stack questions for dstack.

Proposed by a session, not yet asked for by the owner: none.

## Knowledge left in other projects

- iemmixer: the session history of 29-30.10. and `.claude/rules/asio-spike.md`. Ask iemmixer through dstack's channel, or the owner.
- dstack: everything Dante protocol.
