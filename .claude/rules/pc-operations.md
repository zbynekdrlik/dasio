---
paths:
  - "scripts/**"
  - "crates/dasio/src/run.rs"
  - "crates/dasio/src/turnaround.rs"
  - "crates/dasio/src/bursts.rs"
---

# Working on the production PCs

Owner rules from dasio#1. They hold for every change on a PC running dasio.

- **Latency is per receiver.** Judge lateness against each receiver's own
  latency, never against one global value.
- **Read Dante state yourself.** Use netaudio or dcontroller before asking
  anyone.
- **One change per step, judged part by part.**
  - Read `health.ps1` (or the same counters) before and after each change.
  - Have the rollback ready before the change.
- **A "stable" report covers each PC's DAW too.** Report its process,
  `attached=1` and the DAW turnaround, not only the service.
- **Soaks follow a real service.**
  - A song plays out, the DAW stops, there is a calm pause, then the next
    song.
  - No jumping between songs while recording.
- **Jobs started through the remote MCP shell:**
  - start them detached, with no output redirects. A `Start-Process
    -RedirectStandardOutput` job on mbc (9.11.2026 23:21Z) left every
    later MCP call to that PC timing out for hours. Redirect inside the
    job's own script instead (`*> file`);
  - set their priority class yourself, because they start at BelowNormal;
  - write their result to a file.
- **A program with a window never starts from the MCP Shell.** Start it
  with the App tool, which runs it detached in the user's session, and
  expect an elevation prompt for a vendor tool. On iem-snv (10.11.2026)
  Realtek's 8169DiagV2.exe started from the Shell left remote control
  unreachable from 13:36 to 17:48Z, until someone logged in at the desktop
  (over RustDesk). The PC itself and its network stayed up.
- **A network change on a PC with one NIC** (VLAN, binding, IP) runs inside
  one detached dead-man script. The script makes the change, then undoes
  it unless a `keep` file appears within N seconds. Create `keep` only after
  an MCP read through the new path works. Never guess health from "the
  gateway answers": a router answers over any leg. Read the driver's VLAN
  behaviour first: Realtek's PCIe driver unbinds TCP/IP from the base
  adapter at the first VLAN (USB NICs may not). On iem-snv (10.11.2026) a
  VLAN added through the GUI with a gateway-ping watchdog cut the PC off
  for the night.
- **Elevated work runs only what non-administrators cannot change**
  (dasio#28 E19).
  - A bundle goes to an admin-only folder before `install.ps1` runs. That
    script refuses any other place, because a folder made directly under
    `C:\` gives Authenticated Users Modify. Use this recipe:

    ```powershell
    $b = "C:\ProgramData\dasio\bundles\<name>"
    New-Item -ItemType Directory -Force $b | Out-Null
    icacls $b /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
    ```

    Then put the bundle and the instance's config in `$b` (download, or
    copy from the user-side download), check `SHA256SUMS` there, and run
    `$b\install.ps1 -Config $b\device-<n>.toml ...`.
  - The bundle is the main run's `dasio-<sha>` artifact. Get its signed
    blob URL on dev1 with
    `curl -s -o /dev/null -w '%{redirect_url}' -H "Authorization: Bearer $(gh auth token)" https://api.github.com/repos/zbynekdrlik/dasio/actions/artifacts/<id>/zip`
    (`gh api -i` follows the redirect and shows none). On the PC, run
    `curl.exe -o $b\artifact.zip '<url>'` (FOH: `--limit-rate 1M`), then
    `Expand-Archive`. The URL expires about 10 minutes after it is made,
    so make a new one for each PC.
  - Fetch and check first, install at the announced time: after the
    download and the SHA256SUMS check, run the BUNDLE's `dasio.exe
    pc-profile check` (and `pc-profile instance-cpus --instance N --exe
    <exe>`) against the live profile: the new code's verdict with no
    restart (FOH 19.11.2026: 0.1.142's check and CPU step read before band
    restarted). The deploy.ps1 that installs later checks SHA256SUMS again
    and never downloads again: by then the URL has expired (a copy of an
    older deploy.ps1 that curls first fails there).
  - `install.ps1` keeps `C:\ProgramData\dasio` admin-only, with Users
    allowed to read. It refuses a link in that folder, or an item there
    that someone else owns or may change, and names it. Check it by hand
    and fix it there. Never do a recursive `takeown` or `icacls /T` there:
    it would follow a link someone planted.
  - Before `Add-Type`, an elevated ad-hoc script points `TEMP` and `TMP` at
    an admin-only folder, for example `C:\ProgramData\dasio\tmp` made like
    the bundle folder. `Add-Type` compiles into TEMP and loads the DLL
    from there.
- **Zero clicks is the requirement** (owner 9.11.2026).
  - Latency comes down only by removing the causes of the DAW's stalls.
  - Never trade reserve or buffer size for clicks.
  - `DAW turnaround` separates the two causes: `wake_*` is Windows waking
    the driver's thread late; `process_*` is the DAW's callback running
    long (dasio#22).
- **One goal, one issue** (owner 9.11.2026). Nothing agreed lives only in a
  comment of another ticket.
- **A task that belongs to another project is handed over at once**, not
  filed here. That covers dstack, PCs dasio does not run, and the network.
- **Packet captures with pktmon:**
  - Start, restart and stop from one detached script, at fixed UTC times
    agreed with the peers that capture on the other end.
  - pktmon logs each packet at several stack components. Counts come out
    about 8 times too high; the times and ports are what matter.
  - `pktmon etl2txt` is huge (1.1 GB for 4 min of one audio flow). Use
    `pktmon etl2pcap`, pull the pcap at 1 MB/s with `curl.exe --limit-rate 1M -T`,
    and analyse it off the PC with tshark.
  - A remote-shell call ends after 30 s: run the conversion detached too.
- **Never run `install.ps1` straight from the remote shell.** The 30 s
  limit kills it midway. On dsg, 16.11, the old service was already removed
  when it died, and the bridge stayed down until a rerun. Write the bundle's
  `deploy.ps1` (install.ps1 with output into `install.log`, then
  `install.done`) and start it with a SYSTEM scheduled task whose trigger
  lies in the PAST: `Register-ScheduledTask -TaskName <task> -Action
  (New-ScheduledTaskAction ...) -Trigger (New-ScheduledTaskTrigger -Once
  -At (Get-Date).AddDays(-1)) -User SYSTEM -RunLevel Highest`, then
  `Start-ScheduledTask` (no locale date format). Poll for `install.done`,
  then `Unregister-ScheduledTask -TaskName <task> -Confirm:$false`. Never
  `schtasks /create /sc once /st 23:59` + `/run`: that task fires again
  that night. On dsg (17.11.2026 22:59Z) the b0127 and b0128 deploy tasks
  both re-ran, `install.ps1` ran twice in parallel, and b0128's bundle
  config replaced the live one (32 converter taps back to 48, dasio#120).
  A bundle folder can already exist from an earlier deploy with an old
  `install.done` (mbc bundles\b0182 had one from 13.11): delete
  `install.done` (or use a fresh folder) before starting the SYSTEM task,
  or the wait for it ends at once while install.ps1 still runs
  (28.11.2026).
- **A stopped service is not an exited process** (dasio#28, PR #257): `Stop-Service` + `WaitForStatus('Stopped')`
  waits for the SCM state only; the exe can stay open a moment longer (CI 46089759633: `The process cannot access the
  file ... dasio.exe because it is being used by another process`). install.ps1 reads the service's PID and start time
  before the stop (`Get-ServiceProcessInfo`), waits for that process to end (`Wait-ServiceProcessExit`, bounded), and
  copies through `Copy-FileRetry` (sharing/lock violation and access denied only, 10 tries, logged). Any new copy of a
  file a service runs goes through them; `scripts/test_install_ps1.py` guards it.
- **Remote-shell gotchas (dsg, 17.11.2026):** `& script.ps1` is refused
  (script execution disabled): run a helper with `powershell -NoProfile
  -ExecutionPolicy Bypass -File <ps1>`. A config written with PS 5.1's
  `Set-Content -Encoding utf8` gets a BOM: write TOML with
  `[IO.File]::WriteAllText(path, text, (New-Object Text.UTF8Encoding $false))`.
  For a soak, the MCP `Wait` tool caps at 300 s (use 240) and several
  `Wait` calls in one message run at the same time: send them one by one.
- **A dasio PC on two networks uses dstack's layout B** (two-networks.md):
  Dante UNTAGGED on the physical NIC, the LAN as the tagged Realtek Virtual
  Adapter. dasio's NDIS filter (dasio_pacer) cannot bind to a Realtek
  Virtual Adapter (no binding row exists), and on the physical NIC its
  kernel-TX frames leave untagged, so Dante on a VLAN adapter (layout A)
  cannot carry dasio at all. Say this at the start of any two-network plan
  (dsg 18.11.2026: layout A was set up and switched before it surfaced).
  The Dante leg on the physical NIC can take DHCP with a fixed lease (its
  MAC is stable) with `Set-NetIPInterface -IgnoreDefaultRoutes Enabled`
  and metric 900; the LAN VLAN adapter takes a new random MAC at every base
  restart, so its lease is not stable. Opening Realtek's 8169DiagV2 window
  resets the base NIC twice (~5 s link drops each).
  Give that LAN VLAN adapter a STATIC address outside the site's DHCP pool
  (dsg at Poprad: 10.77.8.209/23, switched with a dead-man, keep written
  through the new address): it then survives a reboot or a new random MAC
  without anyone pinning the VMAC (18.11.2026, dasio#184). Dante Virtual
  Soundcard cannot use a Realtek VLAN adapter at all (its interface list
  offers only the physical NIC): a PC whose Dante is DVS needs Dante
  untagged on the physical NIC, or no DVS (resolume-pp 18.11.2026: DVS
  stopped after a move to its Dante70 VLAN adapter; owner: no DVS at PP).
- **An install run as SYSTEM writes SYSTEM's own HKCU** (`HKU\S-1-5-18`,
  which no one logs on with). Anything per-user it sets (dtray's Run value)
  needs a machine-wide place instead: dtray's autostart is one HKLM Run
  value since 0.1.149 (dsg never started dtray at logon before, dasio#211;
  a second dtray in a session exits at once, dtray's mutex `Local\dtray`).
  Never `New-Item -Force` an existing registry key: it drops the key's other
  values.
- **A long remote job** (an installer, dante-leg.ps1, a network change)
  runs as a SYSTEM scheduled task, or with an MCP Shell `timeout` large
  enough (the fleet Shell takes any timeout; a 120 s value cut dsg's
  dante-leg.ps1 run midway, 18.11.2026).
- **A GitHub artifact reaches a PC through its signed blob URL.** Get the
  302 `Location` of `/actions/artifacts/<id>/zip` with the gh token. It lives
  about 10 min. Download it on the PC and check every SHA256SUMS line. The
  file-drop share is behind Cloudflare Access, so a PC gets its login page.
- **A device's service restart re-makes every receiver's flows from it.**
  On band that is iem-mixer's return channels and more; on mbc it is
  stream-obs. Before a band restart, tell camera-box and fohmixer; before
  an mbc restart, tell camera-box and fohmixer. Give a UTC time and keep to it.
  fohmixer works on mbc too (Live installs and updates, sets): announce every
  mbc change to it first (reboot, service restart, Live start or stop, CPU
  affinity, NIC) and ask what it is doing there. On 14.11.2026 mbc Live was
  closed for a Live setup while dasio worked on the PC, and the stream went
  silent with neither side knowing why.
  iemmixer wants no notice of a routine band service restart (~5 s gap,
  11.11.2026): it asks for the times when its gate sees a gap. Tell it
  about a FOH PC restart and about any change on dasio's Dante side
  (channels, latency, device name).
- **After a restart, check that the flows came back in the device's own
  log.** dcontroller can be down: on 10.11 its host lost its Dante address
  in a NIC cutover. Count the `TX flow started` lines since the restart and
  compare them with the count before; there must be no `older than the
  latency` lines. Flow requests logged as "held: our TX clock is not ready
  yet" in the first seconds are normal.
- **After the Dante address is removed and set again, restart dasio-N and
  dlisten**, even if they look healthy. On mbc (14.11.2026, dstack#282) a
  rollback re-set the address with netsh. dasio kept running and its ARC
  still answered direct queries, but its mDNS socket had lost its multicast
  membership. mbc-snv vanished from mDNS, stream-obs could not ask for its
  flow again, and the stream had no program audio for 17 minutes. The same
  fault showed in dtray as "expected, not running" for devices that were
  running. This holds until dstack#282 ships a rejoin. Check the receivers
  (stream-obs) after the restart, not only dasio's log.
- **After a reboot of a PC whose Dante NIC is USB (mbc), check dsync
  first** (19.11.2026): dsync and dasio's in-process follower can start
  before Npcap lists the NIC ("Interface '{GUID}' not found. Available:
  [loopback]" → "Service loop failed" in C:\ProgramData\dsync\dsync.log)
  and neither retries (handed to dstack); the device's media clock then
  never becomes ready, every TX flow request is dropped ("our TX clock was
  not ready within 3 s") and its receivers get silence. dsync is the one
  writer on its box: ask dstack for the dsync restart (peer); restart it
  yourself (`Restart-Service dsync`) only while a receiver is silent and
  dstack does not answer at once, and tell dstack. Then wait for "===
  LOCKED ===" and the device's "TX flow started" lines; receivers still
  see late packets for ~2 min while the clock converges (mbc 19.11:
  stream-obs clean from 01:00:45Z, 2 min after the flow started). Fixes
  pending: dstack#431 (the in-process follower retries), dsync's own.
- **netaudio's `subscription add` cannot query the Yamaha card** ("TX
  channel ... not found", by number and by label, 23.11 and 24.11.2026).
  Subscribe our own receiver with `/opt/pipx/venvs/netaudio/bin/python
  scripts/dante_sub_rx.py <rx-device> <n>=<channel>@<device>` (`<n>=-`
  clears), then verify in dcontroller `/api/devices` and by signal.
  - mDNS discovery misses a device now and then (alex-klavir 26.11.2026: 6 of
    ~14 calls). The script looks for the RX device up to `FIND_TRIES` (4)
    times before it gives up. Still read the RX back in dcontroller after a
    timed on/off; a status of `Unresolved` turns `connected` within ~1 min.
- **A dead-man script undoes its change** when no `keep` file comes. It does
  not only restart the NIC: on alex-nb (26.11.2026) a NIC registry change was
  left in place by a script that only restarted.
