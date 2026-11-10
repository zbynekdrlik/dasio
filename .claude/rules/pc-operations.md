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
- **A device's service restart re-makes every receiver's flows from it.**
  On band that is iem-mixer's return channels and more; on mbc it is
  stream-obs. Before a band restart, tell iemmixer and camera-box; before
  an mbc restart, tell camera-box. Give a UTC time and keep to it.
- **After a restart, check that the flows came back in the device's own
  log.** dcontroller can be down: on 10.11 its host lost its Dante address
  in a NIC cutover. Count the `TX flow started` lines since the restart and
  compare them with the count before; there must be no `older than the
  latency` lines. Flow requests logged as "held: our TX clock is not ready
  yet" in the first seconds are normal.
