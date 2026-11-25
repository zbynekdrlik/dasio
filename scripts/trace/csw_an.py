#!/usr/bin/env python3
"""dasio#207: read one cut of a kernel trace (etl-cut.ps1 rows) around a process's audio output loop.

    csw_an.py <cut file> --proc daw.exe [--tag NAME=image.exe] [--from MS] [--to MS] [--gap MS] [--tid TID]

Times are ms from the cut's -AtUtc. Prints the process's threads by activity, the audio thread's wake-ups and
gaps, and a timeline of the largest gap. The audio thread is the most regular one (median period nearest one
card block), or the one given by --tid.
"""
import argparse
import bisect
import collections
import sys

import cutlib
from cutlib import STATE, WAIT


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("--proc", required=True, help="image name of the process whose audio thread is read")
    ap.add_argument("--from", dest="lo", type=float, default=None)
    ap.add_argument("--to", dest="hi", type=float, default=None)
    ap.add_argument("--gap", type=float, default=3.0, help="list the audio thread's gaps of this many ms and more")
    ap.add_argument("--tid", type=int, default=0, help="the audio thread, when the guess is wrong")
    ap.add_argument("--timeline", type=int, default=400, help="rows of the gap's timeline at most")
    cutlib.add_tag_option(ap)
    a = ap.parse_args(argv)

    cut = cutlib.load(a.cut, kinds="CRSIDTPM", tags=cutlib.parse_tags(a.tag))
    threads, procs, cs, rd, sm, isr, dpc = cut.threads, cut.procs, cut.cs, cut.rd, cut.sm, cut.isr, cut.dpc
    mod, who = cut.mod, cut.who

    pids = list(cut.pids([a.proc]))
    if not pids:
        print("no process", a.proc, "in the cut")
        return 1
    if not cs:
        print("no context switches in the cut")
        return 1
    pid = pids[0]
    mine = {t for t, v in threads.items() if v[0] == pid}
    lo = a.lo if a.lo is not None else cs[0][0]
    hi = a.hi if a.hi is not None else cs[-1][0]
    print(f"cut: switches {len(cs)}, readies {len(rd)}, samples {len(sm)}, isr {len(isr)}, dpc {len(dpc)}; "
          f"{cs[0][0]:.1f} .. {cs[-1][0]:.1f} ms; {a.proc} pid {pid}, {len(mine)} threads")
    cpus = sorted({c[1] for c in cs})
    print("per CPU first switch (ms):", " ".join(f"{c}:{min(x[0] for x in cs if x[1] == c):.0f}" for c in cpus))

    # the card: the ISR routine that fires most regularly
    ic = collections.Counter(r for _, _, r in isr)
    print("ISR routines:", ", ".join(f"{mod(0, r)} x{n}" for r, n in ic.most_common(6)))
    dc = collections.Counter(r for _, _, r in dpc)
    print("DPC routines:", ", ".join(f"{mod(0, r)} x{n}" for r, n in dc.most_common(8)))

    # per thread of the process: switch-ins and run time
    ins = collections.defaultdict(list)     # tid -> [(t_in, cpu, priority)]
    outs = collections.defaultdict(list)    # tid -> [(t_out, cpu, reason, state)]
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if new in mine:
            ins[new].append((t, cpu, npri))
        if old in mine:
            outs[old].append((t, cpu, reason, state))
    run = {}
    for tid in mine:
        tot = 0.0
        o = outs.get(tid, [])
        j = 0
        for t_in, _, _ in ins.get(tid, []):
            while j < len(o) and o[j][0] < t_in:
                j += 1
            if j < len(o):
                tot += o[j][0] - t_in
        run[tid] = tot
    print(f"\n{a.proc} threads by switch-ins:")
    top = sorted(mine, key=lambda t: -len(ins.get(t, [])))[:22]
    for tid in top:
        v = threads[tid]
        n = len(ins.get(tid, []))
        if not n:
            continue
        cp = collections.Counter(c for _, c, _ in ins[tid]).most_common(3)
        pr = collections.Counter(p for _, _, p in ins[tid]).most_common(2)
        print(f"  tid {tid:6} {v[3] or '-':18} base {v[2]:2} pri {pr} start {mod(pid, v[1]):28} ins {n:6} run {run[tid]:8.1f} ms cpus {cp}")

    # the audio thread: the most regular one (median period nearest a card block), else --tid
    def median(x):
        x = sorted(x)
        return x[len(x) // 2] if x else 0.0
    cand = []
    for tid in top:
        tt = [x[0] for x in ins.get(tid, [])]
        if len(tt) < 50:
            continue
        d = [b - c for b, c in zip(tt[1:], tt)]
        cand.append((abs(median(d) - 1.3333), -len(tt), tid, median(d)))
    cand.sort()
    print("\nmost regular threads (median period, ms):", ", ".join(f"{who(c[2])} {c[3]:.3f} x{-c[1]}" for c in cand[:6]))
    audio = a.tid or (cand[0][2] if cand else 0)
    if not audio:
        print("no audio thread found")
        return 1
    print(f"audio thread taken: {who(audio)}")

    tt = ins[audio]
    gaps = [(tt[i + 1][0] - tt[i][0], tt[i][0], tt[i + 1][0]) for i in range(len(tt) - 1) if lo <= tt[i][0] <= hi]
    big = sorted((g for g in gaps if g[0] >= a.gap), reverse=True)
    print(f"\naudio thread switch-ins {len(tt)}, gaps >= {a.gap} ms: {len(big)}")
    for g in big[:15]:
        print(f"  {g[0]:7.2f} ms   from {g[1]:10.3f} to {g[2]:10.3f}")
    if not big:
        return 0
    g = big[0]
    w0, w1 = g[1] - 6.0, g[2] + 6.0
    print(f"\n=== timeline {w0:.3f} .. {w1:.3f} ms (the largest gap {g[0]:.2f} ms) ===")
    # who runs on each CPU, for the readier of an R row
    by_cpu = collections.defaultdict(list)
    for c in cs:
        by_cpu[c[1]].append(c)
    cpu_t = {c: [x[0] for x in v] for c, v in by_cpu.items()}

    def running(cpu: int, t: float) -> int:
        i = bisect.bisect_right(cpu_t.get(cpu, []), t) - 1
        return by_cpu[cpu][i][2] if i >= 0 else -1

    card = ic.most_common(1)[0][0] if ic else 0
    ev = []
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if w0 <= t <= w1 and (new in mine or old in mine):
            ev.append((t, f"cpu{cpu:2} SWITCH in {who(new)} pri {npri}  <- out {who(old)} pri {opri} "
                          f"{STATE.get(state, state)}/{WAIT.get(reason, reason)}"))
    for t, cpu, tid, reason, inc in rd:
        if w0 <= t <= w1 and tid in mine:
            by = running(cpu, t)
            near = [mod(0, r) for (dt, dcpu, r) in dpc if dcpu == cpu and 0 <= dt - t < 0.05]
            ev.append((t, f"cpu{cpu:2} READY  {who(tid)} by {who(by) if by >= 0 else '?'}"
                          + (f" [in DPC {near[0]}]" if near else "") + f" adj {reason}/{inc}"))
    for t, cpu, r in isr:
        if w0 <= t <= w1 and r == card:
            ev.append((t, f"cpu{cpu:2} ISR    {mod(0, r)}"))
    ev.sort()
    audio_only = [e for e in ev if f":{audio}" in e[1] or " ISR " in e[1]]
    shown = ev if len(ev) <= a.timeline else audio_only[: a.timeline]
    if shown is not ev:
        print(f"({len(ev)} rows; showing the audio thread's and the card's only)")
    for t, s in shown:
        print(f"{t:11.3f}  {s}")

    # what the process's threads did inside the gap
    print(f"\n=== inside the gap {g[1]:.3f} .. {g[2]:.3f} ===")
    act = collections.Counter()
    for tid in mine:
        o = outs.get(tid, [])
        j = 0
        for t_in, _, _ in ins.get(tid, []):
            while j < len(o) and o[j][0] < t_in:
                j += 1
            t_out = o[j][0] if j < len(o) else g[2]
            a0, a1 = max(t_in, g[1]), min(t_out, g[2])
            if a1 > a0:
                act[tid] += a1 - a0
    for tid, ms in act.most_common(14):
        print(f"  ran {ms:7.2f} ms  {who(tid)}")
    sc = collections.Counter()
    for t, cpu, tid, ip in sm:
        if g[1] <= t <= g[2] and tid in mine:
            m = mod(pid, ip)
            sc[(tid, m.split("+")[0])] += 1
    print("  samples (thread, module):")
    for (tid, m), n in sc.most_common(24):
        print(f"    {n:4}  {who(tid):44} {m}")
    # the audio thread's own state at the gap's start
    o = [x for x in outs.get(audio, []) if g[1] <= x[0] <= g[2]]
    for x in o[:6]:
        print(f"  audio thread out at {x[0]:.3f} on cpu{x[1]}: {STATE.get(x[3], x[3])}/{WAIT.get(x[2], x[2])}")
    r = [x for x in rd if x[2] == audio and g[1] - 1 <= x[0] <= g[2] + 1]
    for x in r[:12]:
        by = running(x[1], x[0])
        print(f"  audio thread readied at {x[0]:.3f} on cpu{x[1]} by {who(by) if by >= 0 else '?'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
