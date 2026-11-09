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
  - start them detached, with no output redirects;
  - set their priority class yourself, because they start at BelowNormal;
  - write their result to a file.
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
