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
- **Power-plan values go through powercfg** (dasio#128, mbc 16.11.2026):
  `apply` sets an `{active_scheme}\<subgroup>\<setting>` `ACSettingIndex`
  / `DCSettingIndex` DWORD with `powercfg /setacvalueindex|/setdcvalueindex
  scheme_current <subgroup> <setting> <value>` + `powercfg /setactive
  scheme_current` (`pc_profile::powercfg_commands`), never a registry
  write: a value the scheme never stored stayed absent after one. Keep
  every baseline power entry in that shape (a test checks it).
- **The CPU layout is computed** from the PC's logical CPU count
  (`pc_profile::Layout`, dasio#128): system CPUs first (1 per 8, at least
  1: Windows, the Dante NIC's interrupts, rxd on the last of them), then
  each dasio instance (4 CPUs on 16+, else 2), then the DAW. FOH 32/2 =
  0-3, 4-7, 8-11, 12-31; mbc 12/1 = 0-1, 2-3, 4-11; dsg 4/1 = 0, 1-2, 3.
  `dasio pc-profile layout` prints it. In a profile: `computed =
  "system_cpus"` (binary mask), `"dasio_cpus"` (binary mask of every
  instance's CPUs) or `"rxd_cpu"` (dword) on a registry entry,
  `{system}` / `{dasio:N}` / `{daw}` as a process affinity, `instances = N`
  (default 1) in the site profile. Never write a CPU mask by hand where the
  layout gives it.
- **The Dante NIC's RSS range is computed too** (dasio#128, 17.11.2026):
  `computed = "rss_base_cpu"` / `"rss_max_cpu"` / `"rss_cpus"` give
  `*RssBaseProcNumber` / `*RssMaxProcNumber` / `*MaxRssProcessors` as
  strings (`Layout::rss_span`): the system CPUs without CPU 0 (every timer
  DPC, dasio#24) when there are 2 or more, else CPU 0 alone; never
  dasio's or the DAW's (dsg's receive DPCs on its pump CPU: to_asio
  underruns, dasio#120). FOH 1-3, mbc 1, dsg 0. Baseline entries are
  `nic_service = "rt640x64"` only: mbc's USB Realtek (`rtu56cx22x64`) has
  no RSS (`Get-NetAdapterRss` finds nothing), its receive DPCs are the
  USB controller's. Not must-have; `apply` restarts the NIC. FOH's (0-3,
  base 0) and dsg's hand entries in their site profiles win until the
  rollout removes them; a site overrides all three or none (a lone base
  0 with the computed 3/3 reads 0-3 with 3 processors). The range holds
  rxd's CPU on purpose (both are the NIC's receive work). After FOH's
  rollout (3 RSS CPUs, `*NumRssQueues` 4) read `Get-NetAdapterRss` back.
  A site profile naming these needs 0.1.120+.
- **The Dante NIC's driver is a `[[nic_driver]]` entry** (dasio#128,
  17.11.2026), by the NIC's hardware (`hardware_ids` prefixes of its PnP
  hardware IDs, matched up to `&`/`\`), never by the driver bound now:
  `version`, `provider` (checked, WARN + tray count), `inf`, `signer` (the
  catalog's CN) and every `files` name + SHA256 (checked only by
  `install.ps1 -NicDriver <folder>` through `dasio pc-profile nic-driver`,
  which refuses any extra, missing or changed file and any other signer).
  The baseline pins FOH's Realtek 10.79.20.1003 `rt640x64sta.inf` for
  RTL8125 and RTL8168. Realtek serves only its newest release (10.79 is
  gone from realtek.com and absent from the Update Catalog), so the folder
  is a copy of FOH's package (`pnputil /export-driver <oem inf> <folder>`,
  FOH's oem name read with `pnputil /enum-drivers`), put inside the bundle
  folder. `-NicDriver` is opt-in only: the adapter restarts, someone on
  site. `pnputil /add-driver /install` gives the driver to EVERY device
  the INF matches: a second Realtek NIC (often the remote path) restarts
  too, and a running instance loses its NIC (the step is before the
  instance's stop). `hardware_ids` must name the device (`&DEV_`/`&PID_`),
  never a vendor alone; `why` is required. The package's SHA256 are the
  trust anchor; the signer CN only adds Windows' own catalog check. pnputil keeps a newer driver Windows ranks higher (dsg's 10.80):
  install.ps1 refuses after the read-back; remove that one first. A site
  `[[nic_driver]]` with the same `family` wins (dsg measuring 10.80); it
  needs 0.1.121+ on the PC. A new pinned package: read its files' SHA256
  and the catalog's `Get-AuthenticodeSignature` on a PC running it.
- **Reserved CPUs** (dasio#120): the baseline's `ReservedCpuSets`
  (`Session Manager\kernel`, `computed = "dasio_cpus"`, read at boot) keeps
  every thread off dasio's CPUs unless its process asks (CPU sets) or is
  pinned there (affinity wins over CPU sets): Defender's MsMpEng, which
  Lasso cannot pin, had run on dsg's pump CPU. At start the service takes
  its instance's CPUs as its default CPU sets (`cpu_sets`, a bridge also
  the reserved DAW CPUs) when all are reserved, else a WARN ("not
  reserved ... after a reboot") and only Lasso's affinity, as before. Not
  must-have. A site value overrides it (FOH's 4-31; dsg's bridge on 1-3
  needs its own `0e00000000000000`). Judge it after the reboot: the start's
  `reserved CPUs` INFO line, then `engine pump lost` by `cause=`. A site
  profile with `dasio_cpus` needs dasio 0.1.114+.
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
- Tests: a site profile (`parse`/`load`) with a `{nic_class}`/`{nic_device}`
  key needs `nic_guid`, the baseline (`parse_with(.., false)`) does not. A
  test that reads install.ps1 or a source file as text replaces `\r\n`
  first: the Windows runner checks out CRLF. The Windows glue (exit codes,
  `layout`) is tested through the built exe in `tests/pc_profile_cli.rs`.

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
- Windows (WaaSMedic) sets UsoSvc, the Update Orchestrator, back to
  automatic at boot (FOH 8.11 + 15.11.2026, SCM 7040; not seen yet on the
  LTSC 24H2 boxes). The service holds it itself (dasio#158,
  `pc_profile::hold`, 0.1.119+): right before each profile check (the
  first after start, then every 10 min, on the helper CPUs at the thread's
  lowest priority) it sets the start type back to disabled through the SCM
  and stops it (waiting out a start in progress), in process, with a WARN
  `UsoSvc held off (dasio#158): ...`; a reset after a check waits for the
  next one (up to 10 min). No task is installed for it. FOH's hand-made
  `\dasio-usosvc-off` goes only after the dsg reboot proof on #158.
  Another service Windows resets by itself goes on `hold::HELD` (only
  through a reviewed code change, never from a profile) with the same
  `Start` in the baseline (a test keeps them equal). Windows Update still
  works: dasio's update runs (`wu.ps1`, dasio#109) use the Windows Update
  Agent API through `wuauserv` (manual start), not UsoSvc (mbc 13.11: 5
  updates installed with UsoSvc disabled and stopped). Never hold
  `wuauserv`.
- Process Lasso can migrate its ini at start: the `Version` key changes and
  new default keys appear. Diff the ini against the newest backup dasio
  made before a Lasso change (`prolasso.ini.bak-*` / `prolasso.ini.before-*`
  next to the ini). Every rule line unchanged: move the profile's expected
  `Version`. A rule line changed: restore it first (or, when the change is
  wanted, update the profile's entry for it in the same step), then move
  `Version`.
- The two Abletons on FOH (band and master) never start on their own after
  a reboot. The dasio session starts both itself; the launch commands are in
  its local memory (`foh-reboot-ableton-start`), not in the repo. It starts them
  NOT elevated (through explorer.exe, never a direct start from the elevated
  remote shell): an elevated Live blocks fohmixer's hub from its windows
  (17.11.2026). Then it
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

## The full fold and what must not exist (dasio#128, owner 16.11.2026)

- The baseline holds EVERY non-site key of FOH's and mbc's profiles, not
  a hand-picked subset (it had 72 entries while FOH's file had 235; dsg,
  set up from it, still ran Defender, Edge's updater and 216 tasks).
  `pc_profile/tests/fold-keys.txt` lists those keys with generic names; a
  test fails when the baseline misses one. A new proven key on any PC: the
  baseline AND the fixture, in the same step. Site only (stays in a PC's
  file): the active plan's GUID, `dasio-N` ImagePath/Start, the adapter's
  subnet, a literal `Enum\PCI` path, CPU masks the layout gives, Lasso
  rules naming site exes (DefaultPriorities, DefaultAffinitiesEx, ...),
  Lasso's `Version`/`DefaultsLevel`, other tools' tasks, the disk floor.
  The classification table is on dasio#128 (16.11.2026).
  Committing baseline `[[ini]]` lines trips the secret-staging hook
  (`key = '...'` reads as a credential): it is a Lasso key name, commit
  with `# airuleset:secret-ok Process Lasso ini key names`.
- Defender: Windows will not uninstall it, so the baseline holds FOH's
  whole policy set off (`Policies\Microsoft\Windows Defender`); its
  `WinDefend` Start 4 is `check_only` (Windows 11's self-protection
  refuses the write, mbc 10.11), so it stays a WARN there until set by
  hand with Tamper Protection off.
- An unreadable ini file (no Process Lasso) is ONE problem naming how many
  keys it holds, not one per key; `apply` plans one write for it.
- `[[absent]]` entries name something that must not exist: exactly one of
  `exe = '<full path>'` (with `uninstall = ['<uninstaller>', args...]`,
  one `\*\` folder allowed: the newest version folder is run),
  `service = '<name>'` (only `pc_profile::absent::DELETABLE_SERVICES`:
  `sc stop` + `sc delete`; widen the list in code, under review, never in
  a profile), `task = '\<path>'` (a trailing `*` matches the rest of a
  name in that folder: Edge's task names end in a GUID). The check reports
  each one found (WARN + tray count, every 10 min, so a Windows upgrade
  bringing it back shows); `apply` removes exactly what was found and
  prints each removal; what an earlier removal already took with it
  (EdgeUpdate's uninstaller removes its services and tasks) is fine; an
  exe still there after its uninstaller, or an uninstaller still running
  after 10 min (killed), is a FAILED line. Tasks are read from
  `System32\Tasks` (no process), so the bridge's in-process check covers
  absences too. A site profile with `[[absent]]` needs the first dasio
  after 0.1.113 (the one shipping it): an older one refuses the whole
  profile.
- The baseline removes Edge (`setup.exe --uninstall --msedge
  --system-level --verbose-logging --log-file=... --force-uninstall`),
  EdgeUpdate (`/uninstall`), its services edgeupdate / edgeupdatem / MicrosoftEdgeElevationService and
  its two task families. EdgeUpdate's `/uninstall` exits 0 but keeps
  itself while WebView2 (`EdgeWebView`, which stays, owner 16.11) is
  installed (dsg + mbc 16.11), so its entry names a fallback (0.1.117+):
  `remove_dir` (the exe's own folder, only one on
  `pc_profile::absent::REMOVABLE_DIRS`, a parse error otherwise; widen it
  in code, under review) and `stop_processes` (exe file names, stopped
  only while their image is inside that folder). When the exe stays after
  the uninstaller, `apply` prints a `fallback for ...` line, stops each
  named process (`fallback: stopped|left ...`), deletes the folder
  (`fallback: deleted ...`) and looks again; the exe still there is a
  FAILED line. Its tasks and services go by their own entries, which run
  after it.
- Edge's setup refuses `--uninstall --force-uninstall` on Windows 10 LTSC
  (FOH 17.11.2026, EdgeUpdate's log: "Edge not uninstallable per regional
  policy", though FOH's device region is SK and
  `IntegratedServicesRegionPolicySet.json` lists SK: the EEA uninstall is
  Windows 11's). So Edge's entry names `before_uninstall =
  ['HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev\AllowUninstall']`
  (0.1.124+): `apply` writes it (REG_SZ "", the value the known removers
  write) right before the uninstaller, only while the exe is there,
  printing `before the uninstaller of ...: set ...`. Only a value on
  `pc_profile::absent::PRE_UNINSTALL_WRITES` (key, name and data in code;
  widen it under review, a parse error otherwise). The value stays after.
  When Edge still stays, read setup's own reason in
  `C:\ProgramData\dasio\msedge_installer.log` (`--verbose-logging
  --log-file`; without `--verbose-logging` setup logs nothing; should
  `--log-file` be ignored, it is the running account's `%TEMP%`, for
  SYSTEM `C:\Windows\SystemTemp`). Untried further steps the known removers
  use: a placeholder `C:\Windows\SystemApps\Microsoft.MicrosoftEdge_8wekyb3d8bbwe\MicrosoftEdge.exe`,
  a temporary EEA region; decide from that log, not by guessing.
