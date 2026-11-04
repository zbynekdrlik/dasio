---
paths:
  - "crates/dasio/src/pc_profile*"
  - "crates/dasio/src/pc_profile/**"
  - "drivers/ndis-pacer/**"
  - "scripts/install.ps1"
---

# The PC profile (dasio#5)

- Every Windows, NIC, driver-parameter or Process Lasso change on a dasio
  PC goes into that PC's profile in the same step. A change that lives only
  on the PC is lost with the next Windows upgrade or reinstall.
- The profile is site data: it lives at `C:\ProgramData\dasio\pc-profile.toml`
  on the PC, never in the repo. The FOH copy is also kept in the dev box's
  `~/.config/dasio/foh-pc-profile.toml`; keep both the same.
- Check it: `dasio pc-profile check`. Preview and set what differs:
  `dasio pc-profile apply --plan`, then `dasio pc-profile apply`. The
  service checks it at start and every 10 min: each problem is a WARN line
  in the instance log, and dtray's line shows `PC profile: N problems`.
- `[[process]]` entries only check what Process Lasso applied (affinity,
  priority class). The setting itself is Lasso's ini, which the profile
  holds key by key.
- Not covered yet: BCD settings (`disabledynamictick Yes`, `testsigning
  Yes`), checked by hand with `bcdedit /enum {current}`.
