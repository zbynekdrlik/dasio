---
paths:
  - "scripts/trace/**"
---

# Kernel trace of an audio thread (`scripts/trace/`, dasio#207)

Found why master Ableton clicked (25.11.2026). Tools: `dasio-csw.wprp`, `dasio-csw-stk.wprp`, `rec-trace.ps1`
(+ `ableset-osc.ps1`), `tone-trigger.ps1`, `etl-cut.ps1`, readers `csw_an.py` `csw_res.py` `csw_smp.py` `csw_stk.py`
`csw_dump.py` (shared `cutlib.py`). Tests: `python3 -m unittest discover -s scripts/trace` (a synthetic cut,
`fixtures/mini.cut`; CI has no ETW). No site data in the files: process images, tags, addresses, song indexes are arguments.

## Capture

- `wpr -start <profile>!DasioCsw` (memory mode, no file IO while it records), `wpr -stop <out.etl> -skipPdbGen`.
- On a 32-CPU DAW PC while it plays: about 1 M events/s, 38 MB/s. 512 x 1024 KB reaches back about 14 s.
  The ring is shared, not per CPU.
- The trigger: `rec-trace.ps1` listens on UDP and stops the trace; `tone-trigger.ps1` on the PC that listens to the
  tone sends at the first `a watched test tone broke` / `a watched program output looped` line of dasio.log
  (about 0.2 s after the log line). `-GoLive`, `-Songs` (one string: `-Songs 7,3,6,4,5`), `-RecStop` are the AbleSet
  setlist indexes of the set. `rec-trace.ps1` finds `ableset-osc.ps1` beside itself unless `-Osc` is given.
- A reader ends with exit 1 and `no process X in the cut` when `--proc` is not in the cut; a damaged cut (stopped
  mid-line) is read, the bad rows are skipped and counted (`N malformed rows skipped`). `--proc` matches every pid of
  the image (a dead earlier process stays in the P rows).
- Not tracerpt: 3.8 MB/s of CSV (14 min for 231 MB). `etl-cut.ps1` uses the ETW consumer API from PowerShell
  (the offsets of EVENT_TRACE_LOGFILEW it uses: callback at 424, mode at 28). Verified by the header read-back in
  the summary: cpus, pointer size, perf_freq. If these look wrong, the offsets are wrong; do not trust the rows.
- Stack profile `dasio-csw-stk.wprp`: 1.5 GB ring, heavy, seconds only. `etl-cut.ps1` writes K rows only for
  threads whose last wait was WrResource.

## Reading a cut

- The ASIO thread = started in the card's ASIO dll; workers are named `AudioCalc`. `csw_an.py --tid` when the
  guess (the most regular thread) is wrong.
- `Waiting/WrResource` = a kernel lock; the window lock when the wakers are other processes' GUI threads.
  `WrAlertByThreadId` = a user-mode lock.
- R rows (ReadyThread) carry byTid/byPid from the event header; in a kernel trace both read 4294967295, so the readers
  ignore them and take the thread running on that CPU at that time as the waker.
- ISR-MSI is opcode 50.
- `csw_stk.py` joins the kernel and the user part of a stack by its stamp and names system dll exports from
  `C:\Windows\System32` of the PC it runs on.
- A long WrResource wait: follow its holder. `csw_res.py` prints the holder chain under each long wait (`--chain`,
  default 3 steps). It names who made the waiter ready, what that thread had waited on itself, and who made it
  ready. The chain also ends early:
  - at an idle or unknown waker (an interrupt or DPC, or a CPU with no rows);
  - at a thread already in the chain;
  - at a holder that was not waiting (preempted: `Ready, not a wait`; or running since before the cut).
  Switches with one stamp are put to alternate in/out per thread, so a preemption at the stamp of the next
  switch-in is not read as the older wait. FOH 26.11.2026 (1.60.0, one late master block in 200 record starts), all from one cut with no stacks:
  - master's AudioCalc waited 1455 us on WrResource, made ready by master's MainThread;
  - MainThread had waited 2006 us on WrPushLock, made ready by System 976, started in `dxgmms2.sys` (the GPU
    scheduler);
  - so master's window drawing waited on the GPU while holding the window lock.
  A System thread's start module is not in a cut without kernel M rows: read the PC's driver bases
  (`EnumDeviceDrivers` + `GetDeviceDriverBaseNameW`; valid until its next reboot).
- `csw_an.py` lists the 22 threads with the most switch-ins. A card's ASIO thread can fall below them (FOH master:
  6945 against 8700+ per AudioCalc in 2.7 s), and the "most regular" guess then takes an AudioCalc. Find the
  thread started in the card's ASIO dll and pass `--tid`.
- The readers on a PC are copies: copy the repo's version there before reading a cut (FOH's `csw_an.py` had no
  `--period-ms`, 26.11.2026).
- A rec-trace cut (`dasio-csw.wprp`) has no stacks, so `stk_sig.py` and `csw_stk.py` print nothing for it.
  That is no proof that there was no PostMessage; only the stack profile shows the posting module.
- `wpr -setprofint` takes 1221 at the least (0.1221 ms): 1000 fails with 0x80070057, and the trace then runs
  at the default 1 ms.

## Two hard rules

- Never run a trace, or start any process, on a DAW PC during a measurement other than the planned one. That
  includes a waiting trigger trace (`txev-trigger.ps1`): analyse off the box, or stop the trigger first. Only one
  kernel trace runs at a time, so a stack probe also stops the trigger, and the trigger is armed again after.
- During a service, `event-trace.ps1` holds the csw ring on the DAW PC, and mbc runs `tone-trigger.ps1 -Match 'a watched
  output broke|a watched test tone broke|a watched program output looped'`. The trace is saved once, at the first miss the
  hall can hear, and then the script ends. The save writes the whole ring to disk, so a run keeps one save. A run
  counts as no measurement: stop it (`event-trace.stop` flag) before any planned probe or test, and arm it again after.
- Never read or sweep another process's memory on an audio PC (it stopped a production PC on 25.11.2026).
