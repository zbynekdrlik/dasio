---
paths:
  - "crates/dasio/src/pc_profile*"
  - "crates/dasio/src/pc_profile/**"
  - "drivers/ndis-pacer/**"
  - "scripts/install.ps1"
---

# The PC profile (dasio#5)

- **The baseline first (dasio#128, owner 15.11.2026).** Every proven
  setting lives in `config/baseline-pc-profile.toml`, built into dasio and
  merged with each PC's own profile (`pc_profile::load`). A new finding on
  any PC goes into the baseline (with its why and ticket), not into one
  PC's file; a PC's file holds only site data (adapter GUID, CPU layout,
  its own services, overrides). NIC-family settings carry `nic_service`
  (the Dante NIC's driver service, e.g. `rt640x64`); power-plan values use
  `{active_scheme}`. Never set up a new PC by redoing findings one by one.
- **The CPU layout is computed** from the PC's logical CPU count
  (`pc_profile::Layout`, dasio#128): system CPUs first (1 per 8, at least
  1: Windows, the Dante NIC's interrupts, rxd on the last of them), then
  each dasio instance (4 CPUs on 16+, else 2), then the DAW. FOH 32/2 =
  0-3, 4-7, 8-11, 12-31; mbc 12/1 = 0-1, 2-3, 4-11; dsg 4/1 = 0, 1-2, 3.
  `dasio pc-profile layout` prints it. In a profile: `computed =
  "system_cpus"` (binary mask) or `"rxd_cpu"` (dword) on a registry entry,
  `{system}` / `{dasio:N}` / `{daw}` as a process affinity, `instances = N`
  (default 1) in the site profile. Never write a CPU mask by hand where the
  layout gives it.
- **Must-have items** (`must = true`, dasio#128): the Dante NIC's timing
  settings (Realtek PCIe) and `testsigning`. A missing one: the device does
  not start / ends at the 10-min check (the SCM restarts it, the start
  names it again), `dasio pc-profile check|apply` exits 2 with a
  `MUST-HAVE` line, and `install.ps1` refuses before touching the instance.
  A site value for a must-have item stays must-have. A profile that does
  not load counts as must-have too (start refused, exit 2). The start
  checks the whole profile (test signing included); the bridge's 10-min
  in-process check skips `[[bcd]]`/`[[task]]` (no process started from the
  media process). A Dante NIC not found ends the device as well.
  `install.ps1` refuses a PC without a site profile naming the device's
  adapter as `nic_guid` (it prints the GUID to write).
- At rollout, a site entry for the NIC's interrupt affinity must use the
  `{nic_device}` key (or go): a literal `Enum\PCI\...` path (dsg's) is a
  second entry for the same value, and the two fight. Give FOH's profile
  `instances = 2`.
- `computed`, `must`, `instances` and placeholder affinities need dasio
  0.1.107+ on the PC: an older one refuses the whole profile.

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
- A `[[registry]]` entry with `absent_ok = true` accepts an absent value:
  use it for the `Start` of a service that may be uninstalled. Any other
  value is still a problem, and `apply` creates nothing for an absent one.
  Add it to a PC's profile only after that PC runs a dasio that knows it
  (0.1.46+): an older one refuses the whole profile. FOH's
  `GigabyteUpdateService` has been gone since the 10.11.2026 reboot.
- `[[bcd]]` entries (`name = "disabledynamictick"`, `value = "Yes"`) check
  one setting of the running boot entry (`bcdedit /enum {current}`, name
  and value without case). Checked only, never applied: a BCD change takes
  a reboot. Add one to a PC's profile only after that PC runs a dasio that
  knows `[[bcd]]` (0.1.54+): an older one refuses the whole profile.
- `[[task]]` entries (`name = '\fleetbackup-main'`, `enabled = true`,
  `triggers = ["daily 04:00"]`) check one Task Scheduler task. A task's
  triggers read as short lines: `daily HH:MM`, `every Nd HH:MM`,
  `weekly HH:MM`, `time HH:MM`, `boot`, `logon`, with ` every PT2H` for a
  repeat and ` (off)` for a disabled trigger. `absent_ok = true` accepts an
  uninstalled task. Checked only, never applied: the task belongs to its
  tool. Add one to a PC's profile only after that PC runs a dasio that
  knows `[[task]]` (0.1.61+): an older one refuses the whole profile.

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

## CPU sets on a PC with a timer-waiting device (mbc)

- mbc's dasio runs `timing = "normal"`: no spin, timers for the whole wait.
  On 14.11.2026, `pump_cpu = 2` and `rx_cpu = 3` inside a Lasso set of
  CPUs 2-3 made its pump wake ~31 ms late about 300 times an hour. Each
  late wake re-bootstrapped the engine and dropped ~12 blocks
  (`cause=late`, `missed=12`). Removing the two keys, so the threads float
  on the process's CPUs, brought it to 0 in 30 min (dasio#24).
- So on such a PC, give the process its CPUs with Lasso and pin no thread.
  FOH's thread pins (pump/rx/io) belong to its spinning real-time profile.
- After any CPU change, count `engine pump lost` lines per hour by
  `cause=` before and after, not only network loss.
