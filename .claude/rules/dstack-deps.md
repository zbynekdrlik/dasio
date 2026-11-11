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
- `--iface` names the Windows interface alias (if-addrs' FriendlyName),
  compared exactly. `install.ps1` matches it with `-ceq`, never
  `-InterfaceAlias`, which takes wildcards and ignores case.
