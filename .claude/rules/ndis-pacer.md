---
paths:
  - "drivers/ndis-pacer/**"
  - "crates/dasio/src/kernel_tx*"
  - "crates/dasio/src/kernel_tx/**"
---

# dasio's NDIS filter (dasio_pacer)

## Reinstalling it on a PC where dasio runs

- With `--kernel-tx`, each dasio instance holds `\\.\dasiopacer` open. A
  filter whose device is open cannot unload. Stop the dasio services
  first, then `netcfg -u dasio_pacer`, then `install.ps1 -Adapter <NIC>
  -RxdCpu <n>`, then start the services again.
- `install.ps1 -RxdCpu` writes the service's Parameters (RxdEnable,
  RxdCpu). Give the FOH PC's current CPU, read from
  `HKLM\SYSTEM\CurrentControlSet\Services\dasiopacer\Parameters`.
- Run the whole sequence from one scheduled task (Interactive, Highest):
  the MCP shell's call can end while the NIC rebinds.
- Check afterwards: the sys file's hash, `rxd.ps1 status`, and `ktx.ps1
  status` (Ready 1, Failed 0, TxNoSlot 0). Each dasio log must have
  "audio TX goes through dasio's NDIS filter (kernel TX)".

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
