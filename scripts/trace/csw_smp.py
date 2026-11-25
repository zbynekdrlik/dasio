#!/usr/bin/env python3
"""dasio#207: where the 1 ms samples of a trace cut (etl-cut.ps1) fall, per thread group and module.

    csw_smp.py <cut file> --proc a.exe [--proc b.exe ...] [--tag NAME=image.exe] [--tids 7768,11784]

For each given process: samples of the audio threads (AudioCalc, the ASIO thread) and of the main thread by
module, kernel modules listed apart. Also every WrResource wait of any thread of the processes: who waited, how
long, who made it ready.
"""
import argparse
import bisect
import collections
import sys

import cutlib
from cutlib import KERNEL, WAITING, WR_RESOURCE


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("--proc", action="append", required=True, help="image name of a process to read (repeatable)")
    ap.add_argument("--tids", default="")
    cutlib.add_tag_option(ap)
    cutlib.add_audio_option(ap)
    a = ap.parse_args(argv)
    cut = cutlib.load(a.cut, kinds="CRSTPM", tags=cutlib.parse_tags(a.tag))
    threads, cs, rd, sm = cut.threads, cut.cs, cut.rd, cut.sm
    mod, who = cut.mod, cut.who

    for tid in [int(x) for x in a.tids.split(",") if x]:
        t = threads.get(tid)
        if t:
            ss = collections.Counter(mod(t[0], ip).split("+")[0] for _, _, st, ip in sm if st == tid)
            print(f"thread {tid}: {who(tid)} base pri {t[2]} start {mod(t[0], t[1])}; samples: {ss.most_common(8)}")

    two = {p: cut.label(p) for p in cut.pids(a.proc)}
    for pid, tag in two.items():
        audio, main = set(), set()
        for tid, v in threads.items():
            if v[0] != pid:
                continue
            start = mod(pid, v[1])
            if cutlib.is_audio_thread(v[3], start, a.audio_module):
                audio.add(tid)
            if v[3] == "MainThread":
                main.add(tid)
        for name, group in (("audio threads", audio), ("main thread", main)):
            user, kernel, kip = collections.Counter(), collections.Counter(), collections.Counter()
            for _, _, st, ip in sm:
                if st in group:
                    m = mod(pid, ip)
                    if ip >= KERNEL:
                        kernel[m.split("+")[0]] += 1
                        if not m.startswith("ntoskrnl"):
                            kip[m] += 1
                    else:
                        user[m.split("+")[0]] += 1
            print(f"\n{tag} {name} ({len(group)}): samples user {sum(user.values())}, kernel {sum(kernel.values())}")
            print("  kernel modules:", ", ".join(f"{m} x{n}" for m, n in kernel.most_common(12)))
            print("  kernel non-ntoskrnl addresses:", ", ".join(f"{m} x{n}" for m, n in kip.most_common(12)))
            print("  user modules:", ", ".join(f"{m} x{n}" for m, n in user.most_common(14)))

    # every WrResource wait of the processes' threads
    mine = {t for t, v in threads.items() if v[0] in two}
    ins = collections.defaultdict(list)
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if new in mine:
            ins[new].append(t)
    by_cpu = collections.defaultdict(list)
    for c in cs:
        by_cpu[c[1]].append(c)
    cpu_t = {c: [x[0] for x in v] for c, v in by_cpu.items()}
    rdy = collections.defaultdict(list)
    for t, cpu, tid, reason, inc in rd:
        if tid in mine:
            rdy[tid].append((t, cpu))

    def running(cpu: int, t: float) -> int:
        i = bisect.bisect_right(cpu_t.get(cpu, []), t) - 1
        return by_cpu[cpu][i][2] if i >= 0 else -1

    waits = []
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if old in mine and state == WAITING and reason == WR_RESOURCE:
            i = bisect.bisect_right(ins[old], t)
            dur = (ins[old][i] - t) * 1000.0 if i < len(ins[old]) else -1.0
            j = bisect.bisect_right(rdy[old], (t, 99))
            by = who(running(rdy[old][j][1], rdy[old][j][0])) if j < len(rdy[old]) else "?"
            waits.append((t, dur, who(old), by))
    print(f"\nWrResource waits of the processes' threads: {len(waits)}")
    pair = collections.Counter((w, by) for _, _, w, by in waits)
    tot = collections.Counter()
    for _, dur, w, by in waits:
        tot[(w, by)] += max(dur, 0)
    for (w, by), n in pair.most_common(30):
        print(f"  x{n:4} total {tot[(w, by)]:8.0f} us  {w:30} made ready by {by}")
    print("the 25 longest:")
    for t, dur, w, by in sorted(waits, key=lambda x: -x[1])[:25]:
        print(f"  {dur:8.0f} us at {t:10.3f} ms  {w:30} made ready by {by}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
