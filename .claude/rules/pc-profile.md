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
  on the PC, never in the repo. The dev box keeps a copy of each PC's
  profile: `~/.config/dasio/foh-pc-profile.toml` and
  `~/.config/dasio/mbc-pc-profile.toml` (since 10.11.2026). Keep each copy
  the same as its PC's.
- A NIC advanced-property change restarts the adapter.
  - Set it from a detached script at a fixed UTC time. The remote shell
    can ride that NIC: mbc has no other one.
  - The script sets the keys with `-NoRestart`, restarts the adapter once,
    waits for the link and the address, then reads them back.
  - Tell the receivers about 30 s. On mbc, 10.11, the adapter was down 6 s
    and the address was back after 9 s, but a receiver whose flow request
    went unanswered retried only 18 s later. Its holes lasted 19 s.
- Check it: `dasio pc-profile check`. Preview and set what differs:
  `dasio pc-profile apply --plan`, then `dasio pc-profile apply`. The
  service checks it at start and every 10 min: each problem is a WARN line
  in the instance log, and dtray's line shows `PC profile: N problems`.
- `[[process]]` entries only check what Process Lasso applied (affinity,
  priority class). The setting itself is Lasso's ini, which the profile
  holds key by key.
- `[[disk]]` entries (`drive = "C:"`, `min_free_gb = N`) are a floor of
  free space: below it, the check reports a problem. Checked only, never
  applied. Add one to a PC's profile only after that PC runs a dasio that
  knows `[[disk]]` (0.1.15+): an older one refuses the whole profile and
  its checks go off.
- Not covered yet: BCD settings (`disabledynamictick Yes`, `testsigning
  Yes`), checked by hand with `bcdedit /enum {current}`.

## After a reboot

- The service's first check runs about a minute before Process Lasso
  starts. Its power plan and process affinity findings at boot are Lasso's
  pending work, not drift. Judge the second check, 10 min later: it logs
  "fixed" for them.
- A board's firmware can re-install a vendor service at every boot through
  WPBT: the exe in `System32` is rewritten at boot and its `Start` goes back
  to 2, with no SCM 7040 event. Disabling the service does not hold.
  `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager`
  `DisableWpbtExecution` = 1 (DWORD) stops it from the next boot on. The
  profile holds it next to the service's `Start`. Seen on FOH, 9.11.2026
  (dasio#1).
- Process Lasso can migrate its ini at start: the `Version` key changes and
  new default keys appear. Diff the ini against the newest backup dasio
  made before a Lasso change (`prolasso.ini.bak-*` / `prolasso.ini.before-*`
  next to the ini). Every rule line unchanged: move the profile's expected
  `Version`. A rule line changed: restore it first (or, when the change is
  wanted, update the profile's entry for it in the same step), then move
  `Version`.
- The two Abletons on FOH (band and master) never start on their own after
  a reboot. The dasio session starts both itself; the launch commands are in
  its local memory (`foh-reboot-ableton-start`), not in the repo. Then it
  checks that the band Ableton is attached to its dasio instance
  (`attached=1` in the instance log) and that Lasso's affinities hold.
