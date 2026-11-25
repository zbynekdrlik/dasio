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
  A standard NDIS keyword that means the same on every miniport gets NO
  `nic_service`, else a NIC of another driver skips it while the check
  reads fine: `*LsoV2IPv4/6 = "0"` hold every Dante NIC (`must`, with
  `absent_ok` for a NIC without LSO), since mbc's USB Realtek
  (`rtu56cx22x64`) ran LSO on unseen (19.11.2026, dasio#24). Values that
  differ by driver (`*SpeedDuplex`, `*ReceiveBuffers`, ...) keep the
  scope. EEE is the exception by owner rule (20.11.2026: EEE on any rig
  device is a serious fail, 0.1.154): `*EEE` (NDIS standard) and Realtek's
  own `AdvancedEEE` and `EnableGreenEthernet` = "0" hold every Dante NIC
  that has them, `must` + `absent_ok`; those two names exist on Realtek
  drivers only (rt640x64 from FOH's tuning, dasio#1; mbc's rtu56cx22x64
  read 20.11), where 0
  is off. A driver that names EEE otherwise is not covered: read a new
  NIC's `Get-NetAdapterAdvancedProperty -AllProperties` first. A driver-free must-have needs `absent_ok` (a test holds it): one
  driver's keyword must-have on every NIC would stop the device on any
  other NIC. A site copy of such an entry replaces the baseline's whole,
  `absent_ok` included: FOH's, dsg's and mbc's site LSO entries go at
  each PC's next rollout. A Dante NIC whose keys (class key, device key)
  are not found from `nic_guid` (`Reader::nic_found`) is ONE line
  (must-have when a must-have entry sits on its keys or needs its
  driver), and no entry on its keys is read or written, a driver-free one
  included (FOH's site file overrides every NIC entry with a driver-free
  copy). A class key found without `DeviceInstanceID` counts as not found
  (fails closed, never seen on a PC). A NIC found with an
  unreadable driver service gives that line only when driver-scoped
  entries need the driver (must-have only through a driver-scoped
  must-have); its keys are read as ever.
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
- **An instance's CPUs are its own** (dasio#199, owner 19.11.2026: several
  dasio on one PC must not fight, now that Lasso is gone and the keeper
  sets exactly what the profile says). `layout::shares` (WARN + tray count,
  never must-have), in every check; the bridge's in-process check leaves
  `others_affinity` out, so its share shows only in `dasio pc-profile
  check` and a DAW instance's check:
  - a `[[process]]` entry that is not an instance's exe
    (`layout::instance_exe`: `dasio-*.exe` but not `dasio-keep.exe` or
    `dasio.exe`), or `others_affinity`, naming CPUs of an instance
    (`dasio:1..instances` from the layout `resolve` keeps in
    `Profile::layout`, and each instance exe entry's own mask);
  - two instance exe entries on a common CPU;
  - an instance exe entry on the CPUs of two or more layout instances. An
    instance's own entry may reach into the DAW's CPUs (dsg's bridge 1-3 is
    its own DAW).
  install.ps1's CPU step (`dasio pc-profile instance-cpus --instance N
  --exe <instance exe>`, in the baseline step right after the nic_guid
  checks, so before the NIC driver step, `apply` and the service stop; a
  test-clock install skips it) refuses `-Instance N` beyond `instances` or
  with any entry but its own exe's (another instance's included) or
  `others_affinity` on its CPUs, or with its own exe's entry off its CPUs
  (some of them, nothing but them and the DAW's): instance N is the
  layout's `dasio:N`.
  `cpus()` and every loop here are bounded (`for i in 0..=MAX_CPUS`): an
  unbounded `while` hung both CI mutation jobs (PR #200).
- **A second instance on FOH** (the Yamaha AIC128-D replaced by dasio, for
  master Live): FOH runs `instances` 1 while `dasio-2` (master-asio-dante)
  stays disabled. To add it: `instances = 2` (layout 0-3 / 4-7 / 8-11 /
  12-31), its exe on `{dasio:2}`, then master Live and AudioGridderServer
  off 8-11 (both on 8-15 today; 12-15, or another split of 12-31 between
  band and master decided with numbers then), reserved CPUs still 4-31;
  `dasio pc-profile instance-cpus --instance 2 --exe
  dasio-master-asio-dante.exe` passes before the install.
- **The Dante NIC's RSS range is computed too** (dasio#128, 17.11.2026):
  `computed = "rss_base_cpu"` / `"rss_max_cpu"` / `"rss_cpus"` give
  `*RssBaseProcNumber` / `*RssMaxProcNumber` / `*MaxRssProcessors` as
  strings (`Layout::rss_span`): the system CPUs without CPU 0 (every timer
  DPC, dasio#24) when there are 2 or more, else CPU 0 alone; never
  dasio's or the DAW's (dsg's receive DPCs on its pump CPU: to_asio
  underruns, dasio#120). FOH 1-3, mbc 1, dsg 0. Baseline entries are
  `nic_service = "rt640x64"` only: mbc's USB Realtek (`rtu56cx22x64`) has
  no RSS (`Get-NetAdapterRss` finds nothing), its receive DPCs are the
  USB controller's. Not must-have. `apply` writes the values but never
  restarts the NIC (it prints `set ... (takes effect after a restart of
  the Dante NIC)`), and the check reads the registry, not the running driver
  (dasio#26, 0.1.166): `nic_restart::check` compares the last write of the
  NIC's class and device keys with the adapter's last start
  (`DEVPKEY_Device_LastArrivalDate`); keys written more than 10 s after it,
  while a NIC-keyed entry reads as wanted, give ONE non-must problem
  `the Dante NIC's registry settings: written, NIC not restarted (...)`
  (`unreadable (...)` when a time cannot be read). It clears after the NIC
  restarts; not must-have, since install.ps1 applies without restarting and
  refuses only on exit 2. Key-level time: it cannot name the value. FOH's (0-3,
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
  dsg 18.11.2026 (10.79.20 -> 10.79.50.1003, from a dead-man task: the
  old package stays in the driver store, so `pnputil /delete-driver
  <new oem> /uninstall /force` rolls back): the new driver set a second
  NIC's Green Ethernet, Gigabit Lite, Power Saving and Shutdown WOL back
  ON (hold every property of a NIC that matters, the SoundGrid port too,
  in the site profile), and the Dante leg's DHCP client sent no request
  any more (renew hung; switch sniff: nothing on 67/68), so the device
  came up on link-local: give a Dante leg with a reserved lease its
  static address instead (10.77.7.50/24, no gateway).
- **A value in SYSTEM's own hive** (dasio#191, 0.1.137+): `[[registry]]`
  takes `HKU\S-1-5-18\...` besides `HKLM\...`. dasio runs as SYSTEM, so
  that hive is the HKCU of dasio's process, where an ASIO driver dasio
  loads keeps its per-user settings (dsg's SoundGrid ASIO buffer, #190).
  Anything else under `HKU\` (another user's hive, loaded only while that
  user is logged on; `HKU\.DEFAULT`, a lowercase SID) is a problem of its
  entry, never a refused profile (that would stop the device). An older
  dasio reports such an entry as one problem too ("only HKLM\ keys").
- **A device the PC does not need is a `[[device]]` entry** (dasio#181,
  0.1.134+; an older dasio refuses the whole profile): `instance_id` (Device
  Manager's "Device instance path", site data), `name`, `why`, `absent_ok`.
  Fine only when the device is off now (problem 22) AND at every boot
  (CONFIGFLAG_DISABLED), or unplugged with the flag; else a WARN naming the
  state (`on`, `off until the next boot`, `not connected, on when
  connected`). `apply` runs `CM_Disable_DevNode(PERSIST)` and sets the flag
  (a vetoed stop is a FAILED line).
  - **The guard** (`device::judge`, one judge for check and plan): an entry
    that would be disabled (on, off only until the boot, unplugged without
    the flag, or unreadable) is refused, never planned, when disabling it
    can cut a network path: the Dante NIC or a device it hangs on (its
    `CM_Get_Parent` chain); any PRESENT Net-class device
    (`CM_Get_Device_ID_List` class + present filter, whatever its address
    or link: the remote path, layout B's LAN VLAN adapter, a VF under a
    synthetic adapter, a Wintun adapter, KDNIC too), a device in its parent
    chain or below it; a device whose removal stops one (its
    `DEVPKEY_Device_Children` and `DEVPKEY_Device_RemovalRelations`, each
    read whole in one `CM_Get_DevNode_PropertyW` call, followed down as
    Windows removes them, at most `MAX_REACH` 4096 devices).
  - A device with no children or no removal relations answers
    CR_NO_SUCH_VALUE (CONFIGRET 37), not an empty list: read as none
    (runner 19.11.2026; before that fix every entry read as unread).
  - An entry already off is fine and never judged (dsg's KDNIC and Wi-Fi):
    the Net list is read only for an entry that would be disabled. So a
    Net-class device that is on (mbc's KDNIC) is never disabled by `apply`,
    nor is a Bluetooth radio while its PAN/RFCOMM children (Net class) are
    there: disable such a device by hand once (Device Manager, no flag);
    it then reads `off until the next boot`, its Net children are gone, and
    `apply` sets the flag.
  - Not guarded: an unplugged device (both lists hold present devices only,
    so an unplugged adapter, a spare USB NIC, is held off like any other
    device), a dependency outside the device tree, and the moment between
    `plan` and the disable (`disable()` does not judge again).
  - Fail closed: without the NIC's chain nothing is checked or disabled; an
    unread Net list, parents or relations leave the entry alone, its line
    ending `; not disabled: <what was not read>`.
  - `instance_id` holds PnP's charset only (printable ASCII, no space, no
    comma), so the guard's ASCII case-blind compare is exact. A path outside
    it is a parse error: the whole profile is refused (like any parse error,
    the device does not start); a path copied from Device Manager never is.
  - Tests: the decision is portable (`tests/device_guard.rs`, a fake
    tree); the glue's reads are compared with `Get-PnpDevice` /
    `Get-PnpDeviceProperty` on the runner. A device-changing test needs
    `GITHUB_ACTIONS` and `RUNNER_ENVIRONMENT=github-hosted`; no test picks or
    counts `ROOT\KDNIC\0000` (one disables it while the others run).

  Never hold a device with a
  raw `Enum\...\ConfigFlags` `[[registry]]` entry (exact DWORD, clobbers
  other bits, only at boot): dsg's 16 move to `[[device]]` at its next
  deploy. Read a PC's devices with `Get-PnpDevice` (InstanceId, Status,
  Problem) before writing entries.
- **Never reserve the DAW's CPUs on a PC with 2 system CPUs** (mbc
  19.11.2026): ReservedCpuSets confines KERNEL threads too, unlike Lasso's
  catch-all (user processes only). mbc with 2-11 reserved ran its USB NIC's
  work on CPUs 0-1 only: CPU0 79 %, System 71 % of a CPU, kernel TX
  max_out 200, RX packets late and missing at stream-obs. Reverted to the
  baseline's dasio CPUs (2-3); keeping other processes off the DAW's CPUs
  is the keeper's job: the site profile's `others_affinity` (#185, below).
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
  settings (Realtek PCIe), LSO and EEE off on every Dante NIC that has the
  keyword (an absent keyword is fine, dasio#24) and `testsigning`. A wrong or missing one: the device
  does not start / ends at the 10-min check (the SCM restarts it, the start
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
  second entry for the same value, and the two fight. FOH's profile gets
  `instances = 2` only with its second instance (above: master Live moves
  off 8-11 first, dasio#199).
- `computed`, `must`, `instances` and placeholder affinities need dasio
  0.1.107+ on the PC: an older one refuses the whole profile.
- Tests: a site profile (`parse`/`load`) with a `{nic_class}`/`{nic_device}`
  key needs `nic_guid`, the baseline (`parse_with(.., false)`) does not. A
  test that reads install.ps1 or a source file as text replaces `\r\n`
  first: the Windows runner checks out CRLF. The Windows glue (exit codes,
  `layout`) is tested through the built exe in `tests/pc_profile_cli.rs`.
  A process check there reads the exe's own process (`dasio.exe`, running
  while it checks); the runner's integrity level is not pinned, so the
  integrity test asserts exactly one of medium / high holds. Inside
  `pc_profile/tests.rs`, `keep::X` is the TEST module `tests/keep.rs`, not
  `pc_profile::keep`: write `super::keep::X` there (a bare `keep::` broke
  every test build, #185).

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
- `[[process]]` entries (affinity, priority class) are HELD by dasio's
  process keeper since 0.1.135 (dasio#185, owner 18.11.2026: what Process
  Lasso did, dasio does): `dasio pc-profile keep`, run as the SYSTEM task
  `dasio-pc-keep` at boot and every 5 min, one at a time (a task's
  restart-on-failure covers only a failed start, so the repetition brings a
  dead keeper back). install.ps1 copies dasio.exe as `dasio-keep.exe`:
  task off, the keeper ended through its stop file
  `C:\ProgramData\dasio\pc-keep.stop` (it exits within 2 s; killed only
  after 15 s: `Stop-ScheduledTask` left it running in CI), copy with a few
  tries, then in a `finally` the stop file removed, task on and started,
  refused when it does not run or ends at once; uninstall.ps1 removes it
  with the last instance the same way. Every 2 s it
  sets what differs on every running process of the entry's exe, through
  one handle per process (image name checked, so a reused pid is never
  touched), affinity first (a mask naming a CPU the PC lacks is refused)
  and the priority after it, each read back; below normal on the system CPUs,
  outside the media process. A process something sets back 5 times in
  10 min is given up with one line (the 10-min check shows the rest).
  Log: `C:\ProgramData\dasio\pc-keep.log` (1 MB, then `.log.1`).
  `integrity` stays check-only.
- The keeper also holds Lasso's other per-process settings (dasio#185,
  0.1.138+; an older dasio refuses the whole profile): `io_priority`
  (very low | low | normal | high, ntdll ProcessIoPriority),
  `memory_priority` (very low | low | medium | below normal | normal),
  `gpu_priority` (idle | below normal | normal | above normal | high |
  realtime, D3DKMT; gdi32 is loaded only for an entry naming it, never in
  a media process) and `priority_boost = false` (Lasso's
  `ThreadPriorityBoosts=<exe>;0`). Each is set only when read and
  different, then read back; an unreadable one is the check's problem, not
  a set. Lasso's numbers: IO 0-3 = the names in order, memory 1-5, GPU 0-5.
- **The keeper's catch-all** (dasio#185, Lasso's `*,0,0-1` on mbc; needs
  dasio 0.1.141+, an older dasio refuses the whole profile): the site
  profile's top-level `others_affinity = "{system}"` (a hex mask or a
  layout placeholder, as a process `affinity`; parse refuses anything else
  and `0x0`, a placeholder the layout cannot fill refuses the load). Every
  2 s the keeper sets it on every running process that runs on EVERY CPU
  (one on another mask set its own, Lasso's
  DoNotAdjustAffinityIfCustomized, or the keeper set it), affinity only,
  read back, one log line per process (`... (others_affinity)`), inside
  the fight limit; one it cannot open: one `could not keep` line (per
  process id: a protected service started again logs again). Never taken
  (`keep::catch_all_takes`): an exe ANY `[[process]]` entry names (a
  check-only one too), the keeper itself, every dasio exe (`dasio.exe`,
  `dasio-*`), a name without `.exe` (System, Idle, Registry, Memory
  Compression, Secure System, vmmem) and the session-0 core
  `keep::NEVER` (smss, csrss, wininit, services, lsass, winlogon; their
  children are taken; widened only in code). The kernel's threads are
  never touched (that is why it is not `ReservedCpuSets`).
  - A child inherits its parent's mask, as under Lasso: a DAW started from
    a pinned explorer or task begins on the catch-all's CPUs. Only an
    entry's `affinity` moves it (within one pass), so with
    `others_affinity` every DAW needs an affinity entry (`{daw}`), never
    only the baseline's integrity one; what it started before the move,
    and every child of an entry's process (mbc's Ableton Index on Live's
    4-11), keeps that mask: name such a child in an entry to move it
    (FOH: crashpad_handler, max helper).
  - A changed value reaches a process at its next start.
  - The keeper's start line says what it holds, or why not (a mask naming
    CPUs the PC lacks, the PC's CPUs unreadable).
  - The check shows at most ONE problem, `others_affinity: not held:
    a.exe, b.exe ... on every CPU`: processes it takes still on every CPU,
    older than 10 s, distinct exe names (5 named), read with the checking
    process's rights (the service's check runs as SYSTEM like the keeper;
    an admin shell counts fewer). A bridge's in-process check leaves it
    out (it opens every process): `dasio pc-profile check` shows it.
  - The runner test passes over its own two processes only
    (`windows::keep::pass_over`): a whole-list pass pins the runner.
  - Rollout on a PC: dasio 0.1.141+ there first (an older one refuses the
    profile and the device does not start), then the key; read the
    keeper's pass, Live's mask after a start and the late RX before and
    after.
- Removing Lasso (dsg 18.11, FOH 19.11.2026): its uninstaller DELETES its
  Bitsum Highest Performance plan and makes another plan active (dsg: High
  performance, FOH: Balanced). Before the removal copy the active plan
  (`powercfg /duplicatescheme <bitsum> <new guid>`, compare `powercfg
  /query` of both), set the copy active, pin the profile's
  `ActivePowerScheme` to it; after the removal set it active again and
  read `powercfg /getactivescheme`. `apply` prints FAILED ("ProcessLasso.exe
  is still there", uninstaller exit 0) while the uninstaller finishes in
  the background: check again a minute later.
  On a Modern Standby laptop (mbc 19.11.2026) the uninstaller deleted
  EVERY plan but Balanced, the copy too, and CLEARED `ReservedCpuSets`:
  `powercfg /export` Lasso's plan first, `/import` it under a NEW GUID
  after (the deleted copy's GUID is refused), pin that, and set reserved
  CPUs only after Lasso is gone (a second `apply`).
- A player's own PC (alex-nb, dasio#234, 25.11.2026) comes with its own
  Lasso rules: `AutomaticGamingModeProcessPaths=ableton live 12 suite.exe`
  with `GamingChangePowerPlan=true` switched the plan to Balanced at every
  Live start (System log: ProcessGovernor `reset policy scheme`), and
  `DefaultAffinitiesEx=ableton live 12 suite.exe,0,2-11` ran Live on
  dasio's CPUs. The baseline has no entry for those lists: read every
  `Default*` and `*ProcessPaths` line before the install, give Live a
  `[[process]]` entry (DAW CPUs), then remove Lasso. Its GUI rewrites
  prolasso.ini as UTF-16 at start and drops keys it does not know (181 ->
  161 lines); its uninstaller cleared `ReservedCpuSets` there too (a second
  `apply`, a reboot to take it).
- A new PC's other two pieces (alex-nb, 25.11.2026): `install.ps1` sets
  dtray's autostart (HKLM Run) but does not start it, so start it once
  for the logged-on user (`explorer.exe "C:\Program Files\dasio\dtray.exe"`).
  An old DanteSync (`C:\Program Files\DanteSync`, service `dantesync`;
  alex-nb had 1.18.0) is replaced by dsync through dsync's own
  `install.ps1`, read and pinned to a commit (raw URL + sha256). It removes
  the legacy service and folder and Dante Time Sync's leftovers, keeps the
  data dir, and installs Npcap only when it is missing (the NIC untouched).
  Read dsync's `[PTP] LOCK` and dasio's `media clock out of holdover` after.
- Before a PC's Lasso goes: Lasso's `DefaultPriorities` for exes without
  a `[[process]]` entry (FOH: dsend, dlisten above normal, cg below
  normal, dvs_service high) and its I/O, GPU and memory priorities become
  `[[process]]` entries; its catch-all `*` affinity becomes
  `others_affinity` where `ReservedCpuSets` does not cover the DAW CPUs
  (mbc: `{system}`; never reserve the DAW's CPUs there, above). dsg needs
  none of that (ReservedCpuSets 1-3, its only priority rule is
  SGDawNodeService).
  Then the site profile names `no_ini = ['C:\ProgramData\ProcessLasso\config\prolasso.ini']`
  (0.1.135+; an older dasio refuses the whole profile): every `[[ini]]`
  entry for that file, the baseline's 150 and the site's, is left out.
  While a PC still runs Lasso, both set the same values from the profile.
- `[[process]]` also takes `integrity = "low"|"medium"|"high"|"system"`
  (dasio#128, 0.1.126+): the token's level (`OpenProcessToken` +
  `TokenIntegrityLevel`, a limited query handle is enough). Checked only,
  a running process is never de-elevated; an unreadable token is a
  problem. The baseline holds both Lives (`Ableton Live 12 Suite.exe`,
  `AbletonLiveMaster.exe`) at `medium`. A site `[[process]]` for the
  same exe takes over only the fields it sets: FOH's and mbc's affinity
  entries keep the baseline's integrity, each with its own `why`; a
  site turns a baseline field off only by naming another value.
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
  `Start` in the baseline (a test keeps them equal). Held since 0.1.130:
  DoSvc (Delivery Optimization) at 4 too: on FOH 18.11.2026 05:03 CET an
  SPP start was followed by UsoSvc set to automatic and DoSvc's `Start`
  4 -> 3 by a direct registry write (no SCM 7040, so look at the profile
  check's drift line, not only 7040) (dasio#26). DoSvc is a protected
  service (`sc qprotection DoSvc`: WINDOWS LIGHT): the SCM refuses its
  start type even to SYSTEM ("Access is denied. (os error 5)", dsg 0.1.130),
  so the hold writes a refused start type to the service's registry
  `Start` (0.1.131, `hold::registry_fallback`); a protected service may
  also refuse the stop. Prove a held service on dsg first: set its `Start`
  by hand, wait for the next check (10 min) and read the "held off" line
  before deploying to FOH or mbc. install.ps1's `gpupdate /force` (for
  the QoS policies) removes Defender's `DisableAntiSpyware` policy value on
  mbc (24H2, a Local GPO Registry.pol with QoS entries): the install applies
  the profile again after it (0.1.132, dasio#24) and refuses on exit 2;
  a drift seen only in the service's first check after an install points
  at that refresh. Since 0.1.152 the baseline's `DisableAntiSpyware`
  accepts an absent value (`absent_ok`): a Group Policy refresh at boot
  removes it too, and current Defender ignores it (real-time protection
  stays off through `Real-Time Protection\DisableRealtimeMonitoring`; dsg
  19.11, no Defender state event while it was absent, dasio#120). Windows Update still
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
  (17.11.2026; the profile reports it as `process ... integrity: high`).
  Then it
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
  its two task families and `\MicrosoftEdgeUpdateBrowserReplacementTask`
  (Edge's MSI install adds it, dsg 17.11.2026). EdgeUpdate's `/uninstall`
  exits 0 but keeps
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
  Even so setup refused on FOH ("Not a Windows caller", `Uninstall was
  blocked for this product: 532`): its `edge_install_util` allows the
  uninstall only when setup's PARENT process is `dllhost.exe` in Windows'
  own folder (log: `Parent process: ...`, `Uninstall allowed: Windows
  caller`). So the entry names `windows_caller = true` (0.1.125+, exe
  only): `apply` copies `%SystemRoot%\System32\cmd.exe` to
  `%SystemRoot%\SystemTemp\dllhost.exe` (another file there is refused,
  never overwritten; a byte-equal copy a killed apply left is replaced),
  runs `dllhost.exe /d /v:off /c ""<setup>" <args>"`
  (`pc_profile::absent::caller_command`; an uninstaller or argument holding
  `" & | < > ^ % !`, a newline, or an empty or spaced argument is a parse
  error), deletes the copy (also after a failed run; past 10 min the
  caller's tree is ended with `taskkill /T /F`) and looks again. The glue is
  `pc_profile/windows/exe.rs`
  (FOH 17.11.2026: Edge 156 removed, exit 20, a few in-use files left in
  the version folder). Refused on FOH 17.11 and not worth retrying: an
  empty `windir` ("No windir environment variable found", he3als's method
  2), a setup.exe copy under `C:\Windows\Temp`, a SYSTEM cmd.exe from
  System32. Not needed: the placeholder `SystemApps\...\MicrosoftEdge.exe`,
  a region change. Setup's reason is in
  `C:\ProgramData\dasio\msedge_installer.log` (`--verbose-logging
  --log-file`; without `--verbose-logging` setup logs nothing).
