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
- Every Parameters value (RxdEnable, RxdCpu, TxMaxOut, RxRingEnable) is
  read in a query of its own (`pacer_params_read`): a value of another
  type (`reg add` without `/t` makes a REG_SZ) turns off only its own
  feature, shows in `rxd.ps1 status` ParamBad (bit 0 RxdEnable, 1 RxdCpu,
  2 TxMaxOut, 3 RxRingEnable; status v3) and is logged with DbgPrintEx. In one shared
  query it had turned rxd off and left media TX unbounded. Prove it on
  mbc at the next filter install: a REG_SZ RxRingEnable, then ParamBad 8,
  rxd on, `ktx.ps1 status` TxMaxOut 200; then the REG_DWORD back.
- The decisions (parse, match, the ring's producer, steal or not, the port
  set, the NBL chain work through a next-linked node) are
  `core/rx_ring.c`, host-tested and fuzzed in `pacer-core` (100 % of its
  lines); the NDIS/IOCTL glue is `driver/rxring.c`. filter.c and device.c
  get only hook calls. Ring layout: `core/rx_ring.h`. The MSVC host test
  compiles the same interlocked branch the x64 driver runs (one `_MSC_VER`
  branch for both). x64 only: rx_ring.c stops an ARM64 build with
  `#error` (no CI build, no acquire load there) until one is added.
- The receive path parses each frame on the stack with no lock; it takes
  the RX ring lock only for a frame on a port some ring's table names
  (`pacer_rx_ports`: one bit per port, rebuilt under the lock), and does
  nothing while no ring names a port. What keeps a still-named port safe
  from a racing reader is `named()`: the rebuild's clear pass skips every
  port some table still names, so its bit is never cleared (the order of
  setting and clearing adds nothing to that). The lock sits alone on its
  cache lines, and a cache line separates the lock-free port count from
  the Scratch the lock holder writes. Each frame gets its own receive
  time. Readers' events are set after the lock, through a reference of
  their own.
- Dante legs are untagged on every PC (owner, 18.11.2026): a frame with a
  VLAN ID (a tag in the frame, or one the NIC took off; a priority tag,
  VLAN ID 0, counts as untagged both ways) never goes into a ring (status
  Tagged); nor does one whose IP or UDP checksum the NIC
  found bad (BadChecksum). Both go up as ever. A 0.0.0.0 entry matches
  only a frame to a unicast MAC (never a subnet broadcast).
- Steal mode: NBLs that may pend are split (stolen ones returned to the
  miniport, the chain untouched when none is); a chain that cannot pend is
  decided 64 NBLs a segment, then the kept NBLs go up in runs of
  consecutive ones (the whole chain when it is one segment and none was
  taken), each run's last link restored right after. While no attached
  ring steals (copy mode), a chain that cannot pend is only offered, never
  taken, and goes up whole however long.
- The reader's contract (filteruser.h): a non-inheritable handle; stop
  reading before RX_CLOSE or the handle's close (the mapping goes at
  once); RX_CLOSE before a second RX_OPEN on one handle; an orphan (owner
  gone, a duplicated handle alive) stays attached until that handle
  closes.
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
- Verify on mbc first (the trial PC; dsg is PP's production bridge since
  18.11, no trials there), copy mode: Delivered = the flow's packets,
  Full 0, the ring's times against dstack's arrivals, and the longest DPC
  of `dasiopacer.sys` with a ring open; then steal. While a ring names a
  port, every frame of an indication (a segment of 64 for a chain that
  cannot pend in steal mode) waits until all of it has been offered, PTP
  319/320 frames included: compare the PTP follower's jitter with and
  without a ring open. A filter reinstall rebinds the
  NIC, and mbc's only NIC carries the remote path: a dead-man script at a
  fixed time, or someone at mbc. FOH only with someone on site.
- `pacer-driver` needs `pacer-core`: an MSVC warning in a host test skips
  the driver build (C4310: a cast that truncates a constant, PR #216).

## Changing a status struct

- `ktx.ps1 status` splits the reply into records of a fixed field count.
  A new field in `DASIO_KTX_STATUS` (or the RXD/PACER ones) needs the
  script's name list and the version constant changed in the same commit.
- `rxd.ps1` and `rxring.ps1` split by each record's own Version
  (`scripts/records.ps1`, tested in `tests/test_records.ps1`), so new
  scripts read an older filter: a new version appends its fields behind a
  `// Version N:` marker in the struct, the script's `$layouts` gets the
  version, and the struct's `C_ASSERT` its size;
  `scripts/test_pacer_status.py` ties names, versions and sizes to
  `filteruser.h`.

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
  txset`) bounds the frames out, 1..4095 (4096 is every slot);
  `install.ps1` writes 200, the
  value measured on FOH (0 failures, 0 holes overnight). Media TX is never
  unbounded since 0.1.152: a missing, unreadable, 0, 4096 or larger
  Parameters value is 200 (all but missing flagged in ParamBad and logged
  to the debug log), and `txset` refuses 0 and 4096 or more. Frames over the bound
  go to the socket and count in `TxNoSlot`.
- Copy the WHOLE `drivers/ndis-pacer/scripts` folder to a PC, never single
  scripts: `rxd.ps1` and `rxring.ps1` load `records.ps1` beside them since
  0.1.152, and without it every action of theirs fails (`install.ps1
  -RxdCpu`, `health.ps1`'s pacer status). The `dasiopacer-<sha>` artifact
  (sys, inf, cat, cer) carries no scripts.

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
