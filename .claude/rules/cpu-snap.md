---
paths:
  - "crates/dasio/src/cpu_snap.rs"
  - "crates/dasio/src/cpu_snap/**"
---

# The burst line (cpu_snap): who ran, what the disks did

At a burst of missed DAW blocks the device logs one `who ran since the
last CPU sample` line. It covers a span of at most ~1 s before the burst
(`span_ms`). Read it like this (dasio#22, 24.11.2026):

- **`procs=`** lists the top 8 processes by CPU ms in the span. Divide by
  `span_ms` for cores: band Live at ~4.3-5.3 is ordinary playback on FOH,
  and it rose to 5.8-7.0 at the record-start bursts. A process new since the
  last refresh (every 10 s) counts all its time.
- **`disks=<n>:r<KiB>/w<KiB>/n<requests>/q<depth>`** shows each physical disk
  (`\\.\PhysicalDrive<n>`, 0..15). Its bytes and completed requests are
  counted in the span; `q` is read at the burst's sample, i.e. after the
  stall.
  - Disks are matched by number only, and each is opened, read and closed
    per sample, so no handle blocks a USB disk's removal.
  - A disk whose byte counters went back is left out of that span.
  - There is no busy time: `IdleTime` is booked only when the next request
    starts, and `ReadTime`/`WriteTime` carry no documented unit.
- **`io=name(pid)=r<KiB>/w<KiB>/o<KiB>`** lists the top 8 processes by bytes
  read plus written (file and device IO, cached reads included). Socket
  traffic is in `o`: band Live streams to AudioGridder there.
  - Only a process with readable counters in BOTH samples is counted, so a
    newly opened process never shows its lifetime bytes. A process that moved
    under 1 KiB read and under 1 KiB written is not listed.
- **`daw_threads=`** names band Live's threads: `AudioCalc` are its audio
  workers, and `dasio` is its ASIO callback thread (named by our driver).
  `MainThread` is its GUI/main thread.
- **The 10-min cost line** carries `io_refused` (processes whose counters
  Windows refused) and `disks` (disks read). The sampler costs ~3.3 ms mean
  and ~26 ms max, where the max is the refresh.

Reading the lines: `C:\dasio-runs\an-window.ps1` cuts lines at 420 chars,
which drops `disks=`/`io=`. Read them raw with `C:\dasio-runs\burst-read.ps1
-From <ts> -To <ts>`, or with a FileShare `ReadWrite,Delete` reader.

Never run anything on FOH during a measured span. Every MCP shell call
starts a process, and a process start on FOH stalls band Live: 61-83 blocks
at a script launch, and 36-64 for a log read mid-run (24.11.2026).
