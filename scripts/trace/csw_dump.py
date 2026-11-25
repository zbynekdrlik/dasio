#!/usr/bin/env python3
"""dasio#207: every row of a trace cut (etl-cut.ps1) between two times, with names.

    csw_dump.py <cut file> FROM_MS TO_MS --proc daw.exe [--tag NAME=image.exe] [--cpus 8-15 | --cpus 3] [--all] [--no-samples]

Switches on the given CPUs (any process), wake-ups of the process's threads (any CPU), samples and DPCs on the
given CPUs. --all also lists wake-ups of other processes' threads made on those CPUs.
"""
import argparse
import sys

import cutlib
from cutlib import STATE, WAIT


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("lo", type=float)
    ap.add_argument("hi", type=float)
    ap.add_argument("--proc", required=True, help="image name of the process whose wake-ups are listed")
    ap.add_argument("--cpus", default="8-15",
                    help="the CPUs to list: one (3) or a range (8-15). The default is the DAW CPUs of one PC layout; "
                         "give your own")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--no-samples", action="store_true")
    cutlib.add_tag_option(ap)
    a = ap.parse_args(argv)
    try:
        c0, _, c1 = a.cpus.partition("-")
        c0 = int(c0)
        c1 = int(c1) if c1 else c0
    except ValueError:
        ap.error(f"--cpus wants N or A-B, got {a.cpus!r}")
    if c1 < c0:
        ap.error(f"--cpus range {a.cpus!r} runs backwards")
    cut = cutlib.load(a.cut, kinds="CRSDTPM", window=(a.lo, a.hi), tags=cutlib.parse_tags(a.tag))
    mod, who = cut.mod, cut.who

    cut.report_skipped()
    pids = cut.need_pids([a.proc])
    if pids is None:
        return 1
    mine = cut.threads_of(pids)
    rows = ([(r[0], "C", r) for r in cut.cs] + [(r[0], "R", r) for r in cut.rd]
            + [(r[0], "S", r) for r in cut.sm] + [(r[0], "D", r) for r in cut.dpc])
    rows.sort(key=lambda r: r[0])
    on = dict(cut.last_on)
    print("running at the start:", ", ".join(f"cpu{c}: {who(on.get(c, -1))}" for c in range(c0, c1 + 1)))
    for ms, k, r in rows:
        cpu = r[1]
        if k == "C":
            _, _, new, old, npri, opri, reason, state, waited = r
            on[cpu] = new
            if c0 <= cpu <= c1 or new in mine or old in mine:
                print(f"{ms:10.3f} cpu{cpu:2} SWITCH in {who(new)} pri {npri} <- out {who(old)} pri {opri} "
                      f"{STATE.get(state, state)}/{WAIT.get(reason, reason)} waited {waited} us?")
        elif k == "R":
            tid = r[2]
            if tid in mine or (a.all and c0 <= cpu <= c1):
                print(f"{ms:10.3f} cpu{cpu:2} READY  {who(tid)} by {who(on.get(cpu, -1))}")
        elif k == "S":
            if not a.no_samples and c0 <= cpu <= c1:
                tid, ip = r[2], r[3]
                tp = cut.threads.get(tid, (0,))[0]
                print(f"{ms:10.3f} cpu{cpu:2} SAMPLE {who(tid)} at {mod(tp, ip)}")
        elif k == "D":
            if c0 <= cpu <= c1:
                print(f"{ms:10.3f} cpu{cpu:2} DPC    {mod(0, r[2])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
