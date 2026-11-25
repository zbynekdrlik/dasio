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
  (about 0.2 s after the log line). `-GoLive`, `-Songs`, `-RecStop` are the AbleSet setlist indexes of the set.
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
- ReadyThread rows carry no waker: the waker is the thread running on that CPU at that time (the tools say so).
- ISR-MSI is opcode 50.
- `csw_stk.py` joins the kernel and the user part of a stack by its stamp and names system dll exports from
  `C:\Windows\System32` of the PC it runs on.

## Two hard rules

- Never run a trace, or start any process, on a DAW PC during a measurement other than the planned one.
- Never read or sweep another process's memory on an audio PC (it stopped a production PC on 25.11.2026).
