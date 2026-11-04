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
