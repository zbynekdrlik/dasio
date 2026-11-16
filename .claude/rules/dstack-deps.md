---
paths:
  - "crates/dasio/Cargo.toml"
  - "crates/dasio/src/place.rs"
  - "crates/dasio/src/run.rs"
  - "scripts/install.ps1"
---

# dstack dependencies and the device's place

- A dstack crate that brings cpal (`dstack-endpoint`) goes under
  `[target.'cfg(windows)'.dependencies]`. On the Linux CI runner cpal needs
  ALSA's headers, which the jobs do not install. Keep the decision code it
  feeds portable (a closure in `place.rs`), so Linux tests and the mutants
  job still cover it (dasio#89).
- `DeviceConfig`'s `arc_port`, `cmc_port`, `flows_port` and `process_id` are
  plain `u16` with serde defaults: "absent" is visible only in the TOML
  text. `place::device_config` reads the table for that.
- Any one of those four keys in a PC's config turns the slot pick off for
  that instance. To let a PC pick (mbc beside ConMon), remove all four
  lines from its config at deploy.
- `iface = "<alias>"` in a device.toml (dasio's own key, dtray's settings page writes it, dstack#246) wins over `--ip`/`--iface`; `place::device_config` removes it before dstack's `DeviceConfig` (deny_unknown_fields) reads the file.
- `--iface` names the Windows interface alias (if-addrs' FriendlyName),
  compared exactly. `install.ps1` matches it with `-ceq`, never
  `-InterfaceAlias`, which takes wildcards and ignores case.
- `--iface` takes the interface's address only when it is not link-local: at boot Windows puts APIPA (169.254.x.y) on a DHCP NIC until the lease comes, and a device started on it was unreachable until a manual restart (dsg, 15.11.2026, dasio#121). The service keeps waiting and logs "only its link-local address"; after `place::LINK_LOCAL_AFTER` (60 s) without a lease it takes the link-local address with a WARN (a Dante network without DHCP). `--ip 169.254.x.y` and `dasio run` take it at once.

## Checking a device's audio from outside (sine checker)

- Read levels first: dcontroller `GET /api/meters/<device>` gives each TX/RX channel's dBFS (~1 s frames), for dstack devices too.
- `check <rx> sine <hz>` works only on dstack-dev's Unix control socket: dstack-dev on Windows has none. Run it on dev1 (Dante VLAN `enp38s0.70`) with `--clock dsync` (dev1's dsync follows the FOH card; `--clock system` makes every packet late, no sudo for 319/320).
- Subscribe 2 channels at most. A 16-channel test receiver ran band-asio-dante out of TX flows ("No more flows (Tx)").
- Two channels on separate flows separate the cases: a sender stall breaks both, a receiver's own lateness only one (dev1 runs at normal priority).
- Live's Preferences Test Tone switches off when the Settings window closes.
- `install.ps1` swaps `dtray.exe`, but the dtray already running in the user's session stays the old version until it restarts. After a deploy, end that dtray process and start the new one with the MCP App tool (a window program never starts from the Shell). `dtray --settings` opens the per-instance settings window (dstack#246). Its Apply writes device.toml through an elevated helper, restarts `dasio-N` and leaves `dtray-apply.result` in the instance folder.

## Moving the dstack tag

- Move all six dstack git deps together and update the lock for those packages only: `cargo update -p dstack -p dstack-audio-io -p dstack-asio -p dlink -p dstack-endpoint -p dstack-resample` (a lock refresh, not a build). Then read the tag's CHANGELOG "Consumers" line and port the API it names.
- Write a dstack issue as "dstack issue N" in commit messages, never "#N": the commit gate reads a bare "#N" as a dasio ticket and refuses the commit when that number does not exist here (0.1.108, dstack issue 325).
