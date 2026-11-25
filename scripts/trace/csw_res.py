#!/usr/bin/env python3
"""dasio#207: the waits of a process's audio threads on a lock, from a trace cut (etl-cut.ps1).

    csw_res.py <cut file> --proc daw.exe [--tag NAME=image.exe] [--min-us 100] [--tids 996,11784] [--rows 60]

Every switch-out of an audio thread (named AudioCalc, or started in an ASIO driver dll) that is not the normal
wait for work (UserRequest), with how long it stayed out, who made it ready, and the thread's last sample before.
"""
import argparse
import bisect
import collections
import sys

import cutlib
from cutlib import STATE, WAIT, WAITING, USER_REQUEST


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("--proc", required=True, help="image name of the process whose audio threads are read")
    ap.add_argument("--min-us", type=float, default=100.0)
    ap.add_argument("--tids", default="")
    ap.add_argument("--rows", type=int, default=60)
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
            print(f"thread {tid}: {who(tid)} base pri {t[2]} start {mod(t[0], t[1])}; samples: {ss.most_common(6)}")
        else:
            print(f"thread {tid}: not in the thread list")

    pid = next(iter(cut.pids([a.proc])), -1)
    audio = set()
    for tid, v in threads.items():
        if v[0] != pid:
            continue
        start = mod(pid, v[1])
        if cutlib.is_audio_thread(v[3], start, a.audio_module):
            audio.add(tid)
    print(f"{a.proc} pid {pid}: {len(audio)} audio threads")

    by_cpu = collections.defaultdict(list)
    for c in cs:
        by_cpu[c[1]].append(c)
    cpu_t = {c: [x[0] for x in v] for c, v in by_cpu.items()}

    def running(cpu: int, t: float) -> int:
        i = bisect.bisect_right(cpu_t.get(cpu, []), t) - 1
        return by_cpu[cpu][i][2] if i >= 0 else -1

    ins = collections.defaultdict(list)
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if new in audio:
            ins[new].append(t)
    rdy = collections.defaultdict(list)
    for t, cpu, tid, reason, inc in rd:
        if tid in audio:
            rdy[tid].append((t, cpu))
    smp = collections.defaultdict(list)
    for t, cpu, tid, ip in sm:
        if tid in audio:
            smp[tid].append((t, ip))

    kinds = collections.Counter()
    found = []
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if old not in audio or state != WAITING or reason == USER_REQUEST:
            continue
        i = bisect.bisect_right(ins[old], t)
        back = ins[old][i] if i < len(ins[old]) else None
        dur = (back - t) * 1000.0 if back is not None else -1.0
        kinds[(WAIT.get(reason, reason), dur >= a.min_us)] += 1
        if dur >= a.min_us or dur < 0:
            j = bisect.bisect_right(rdy[old], (t, 99))
            r = rdy[old][j] if j < len(rdy[old]) else None
            by = who(running(r[1], r[0])) if r else "?"
            s = smp[old]
            q = bisect.bisect_right(s, (t, 1 << 70)) - 1
            last = f"{mod(pid, s[q][1])} ({(t - s[q][0]):.2f} ms before)" if q >= 0 else "-"
            found.append((dur, t, who(old), WAIT.get(reason, reason), by, last))
    print("switch-outs of audio threads that are not the wait for work, by reason (long = at least "
          f"{a.min_us:.0f} us):")
    for (k, long_), n in sorted(kinds.items(), key=lambda x: -x[1]):
        print(f"  {k:20} {'long ' if long_ else 'short'} x{n}")
    print(f"\nthe long ones ({len(found)}), longest first:")
    for dur, t, w, k, by, last in sorted(found, reverse=True)[: a.rows]:
        print(f"  {dur:9.0f} us at {t:10.3f} ms  {w:28} {k:18} made ready by {by:34} last sample {last}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
