# ndis-pacer

An NDIS 6 lightweight filter for the Dante NIC of a Windows PC running dasio.

**Why.** Windows sends all frames of a single-queue NIC in one FIFO. When any
application sends a burst (a 1.5 MB HTTP answer), every Dante packet sent
behind it waits until the burst is on the wire: ~5 ms at 2.5 Gbit/s
(measured on the FOH PC and, with a controlled TCP burst, on a second PC;
dasio#1). Linux avoids this with BQL and fq_codel. TCP pacing, bbr2, QoS
policies, NIC buffer sizes and Npcap sends were measured and do not remove
the wait.

**What it does.** Priority frames (IPv4/IPv6 DSCP 46 or 56, PTP over
Ethernet, PTP over UDP 319/320) go to the miniport at once. All other frames
are paced in time: a token bucket at 85 % of the link speed with a burst of
one timer tick (~0.5 ms, Windows' high-resolution timer), so at most one
tick of other data sits ahead of a priority frame. What does not fit waits
in the filter, in order, for the timer or a send completion. (A cap on bytes
"in flight" does not work: the Realtek miniport completes a whole burst
before it is on the wire; measured on iem-snv, dasio#1.)

**Layout.**
- `core/` the decisions (classifier, budget queue), plain C with no kernel
  types; `tests/` their unit tests (CI: gcc with sanitizers and a coverage
  floor on Linux, MSVC on Windows).
- `driver/` the NDIS glue, from Microsoft's NDIS LWF sample.

**License.** This directory is a separate program from the rest of dasio:
it derives from Microsoft's Windows-driver-samples
(network/ndis/filter), which is under the Microsoft Public License
(MS-PL), so the whole directory is under the MS-PL (see LICENSE).
