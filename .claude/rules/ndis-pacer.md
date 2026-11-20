---
paths:
  - "drivers/ndis-pacer/**"
  - "crates/dasio/src/kernel_tx*"
  - "crates/dasio/src/kernel_tx/**"
---

# dasio's NDIS filter (dasio_pacer)

## dasio never runs without it (dasio#125, owner 15.11.2026)

- Every instance on a real clock sends audio through the filter: no
  `--kernel-tx` / `-KernelTx` any more. The device refuses to start when
  `\\.\dasiopacer` cannot be opened, and `install.ps1` refuses an
  instance whose adapter has no `dasio_pacer` binding. Only `-TestClock`
  (CI) runs without it.
- A new PC: Secure Boot off, `bcdedit /set testsigning on` + reboot, the
  filter (`drivers/ndis-pacer/scripts/install.ps1 -Adapter <Dante NIC>`),
  then dasio. mbc and dsg ran without it until 15.11.2026.

## Reinstalling it on a PC where dasio runs

- Each dasio instance holds `\\.\dasiopacer` open. A
  filter whose device is open cannot unload. Stop the dasio services
  first, then `netcfg -u dasio_pacer`, then `install.ps1 -Adapter <NIC>
  -RxdCpu <n>`, then start the services again.
- `install.ps1 -RxdCpu` writes the service's Parameters (RxdEnable,
  RxdCpu). Give the CPU layout's rxd CPU (`dasio pc-profile layout`: the
  last system CPU, FOH 3, dsg 0); dasio's baseline checks and sets it.
- Run the whole sequence from one scheduled task (Interactive, Highest):
  the MCP shell's call can end while the NIC rebinds.
- Check afterwards: the sys file's hash, `rxd.ps1 status`, and `ktx.ps1
  status` (Ready 1, Failed 0, TxNoSlot 0). Each dasio log must have
  "audio TX goes through dasio's NDIS filter (kernel TX)".

## Deferred receive (rxd)

- `rxdDpc` indicates at most 64 NBLs a run (`RXD_SLICE`) and queues itself
  again while more waits; one run at a time (`RxdRunning` under the Lock),
  so slices go up in order even with no `RxdCpu` (dasio#27). Since then
  `rxd.ps1 status` MaxBatch is at most 64 and DpcRuns counts slices.
- The DPC glue has no unit test (NDIS types, kernel only). Verify a change
  on dsg with rxd on: MaxBatch, DpcRuns rising, the longest DPC of
  `dasiopacer.sys` (xperf/WPR), and dcontroller late 0 at its receivers.

## Kernel RX rings (dasio#27 B23)

- Off unless the service's `Parameters\RxRingEnable` is 1 (REG_DWORD, read
  with RxdEnable when the driver loads: a reinstall or reboot applies it, a
  NIC pause does not). Off: `IOCTL_DASIO_RX_OPEN` answers
  STATUS_NOT_SUPPORTED and the receive path reads one count.
- The decisions (parse, match, the ring's producer, steal or not) are
  `core/rx_ring.c`, host-tested and fuzzed in `pacer-core` (100 % of its
  lines); the NDIS/IOCTL glue is `driver/rxring.c`. filter.c and device.c
  get only hook calls. Ring layout: `core/rx_ring.h`.
- A ring belongs to its handle and its process: mapped in RX_OPEN's
  context, unmapped in RX_CLOSE (owner only) or IRP_MJ_CLEANUP. A last
  close from another process leaves the memory allocated and mapped (a
  leak, never a free under a live mapping).
- A pause or detach of the filter detaches every ring (`rxring.ps1 status`
  Attached 0, the ring's detached word 1, its event set): the reader closes
  and opens again. IRP_MJ_CLOSE frees a ring an RX_OPEN on an overlapped
  handle attached after the cleanup.
- An older filter answers codes 23-26 with success and no output (device.c's
  default case): a reader checks the output's size.
- Verify on dsg first, copy mode: Delivered = the flow's packets, Full 0,
  the ring's times against dstack's arrivals, and the longest DPC of
  `dasiopacer.sys` with a ring open (the RX ring lock is held while a whole
  indication is parsed); then steal. FOH only with someone on site (a
  filter reinstall rebinds the NIC).
- `pacer-driver` needs `pacer-core`: an MSVC warning in a host test skips
  the driver build (C4310: a cast that truncates a constant, PR #216).

## Changing a status struct

- `ktx.ps1 status` splits the reply into records of a fixed field count.
  A new field in `DASIO_KTX_STATUS` (or the RXD/PACER ones) needs the
  script's name list and the version constant changed in the same commit.

## Media TX

- Media TX shares the test stream's slots and NBL pool, and completes
  through `ktxDone`.
- Two FOH devices send about 150 000 frames a second. With Realtek 10.79
  the miniport sometimes holds completions for milliseconds, so the slot
  count is sized for ~27 ms (`KTX_SLOTS`).
- When the slots run out, frames go back to the caller's UDP socket. The
  status counts them in `TxNoSlot`.
- Do not let media TX use every slot. With up to ~448 frames out, Realtek
  10.79 failed frames with NDIS_STATUS_RESOURCES (0xC000009A), and each
  failure was a hole at the receivers. `TxMaxOut` (Parameters, or `ktx.ps1
  txset`) bounds the frames out; `install.ps1` writes 200, the value
  measured on FOH (0 failures, 0 holes overnight). Frames over the bound
  go to the socket and count in `TxNoSlot`.

## Test PCs and NIC drivers

- A test PC runs the current filter build or none, never a leftover one.
  Record which on the ticket.
- On a PC with one NIC, record on the ticket why, before any NIC driver
  install or uninstall. After it, ping the gateway with 1472-byte packets
  and see 0 % loss before leaving the PC.
- After a `/force` uninstall, never leave a PC on a fallback driver without
  saying so on the ticket.
- Never turn interrupt moderation off on a USB NIC. An AX88179 lost the
  PC's network that way (dasio#1, 5.11.2026). Middle is the known-good
  setting.

## Parsing the filter's replies

- A walk over a reply must end even when a size is wrong. A mutant that
  stopped `parse_status` advancing grew its Vec until the mutation runner
  ran out of memory; CI showed only "The runner has received a shutdown
  signal" ~80 s into `cargo mutants` (three times, 13.11.2026). Bound the
  walk by a count a test can check (`MAX_STATUS_RECORDS`), not by an
  iteration cap equal for valid input (that is an equivalent mutant).
