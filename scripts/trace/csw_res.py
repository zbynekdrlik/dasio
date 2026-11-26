#!/usr/bin/env python3
"""dasio#207: the waits of a process's audio threads on a lock, from a trace cut (etl-cut.ps1).

    csw_res.py <cut file> --proc daw.exe [--tag NAME=image.exe] [--min-us 100] [--tids 996,11784] [--rows 60]

Every switch-out of an audio thread (named AudioCalc, or started in an ASIO driver dll) that is not the normal
wait for work (UserRequest), with how long it stayed out, who made it ready, and the thread's last sample before.

Under each long wait, the lock's holder chain (--chain steps, 0 = off): the thread that made the waiter ready, what
it had waited on itself just before and who made it ready, and so on back. A lock holder that waits while it holds
the lock (FOH 26.11.2026: master's MainThread held the window lock and waited 2 ms on a push lock of the GPU
scheduler's thread, dxgmms2.sys) shows as the next step.
"""
import argparse
import bisect
import collections
import sys

import cutlib
from cutlib import STATE, WAIT, WAITING, USER_REQUEST


def order_switches(ev: list) -> list:
    """One thread's switches (t, 1 = in / 0 = out, state, reason) in time order, ins and outs alternating.

    Switches with the same stamp have no order of their own (the cut is sorted by its whole rows, CPU first), so
    within a stamp they are put to alternate after the switch before it: an out-then-in at one stamp is a
    migration, an in-then-out a run too short to measure. Picking the earlier out of the two would report a wait
    that ended before the thread ran."""
    ev = sorted(ev, key=lambda e: e[0])
    out, i, last = [], 0, None
    while i < len(ev):
        j = i
        while j < len(ev) and ev[j][0] == ev[i][0]:
            j += 1
        group = ev[i:j]
        ins = [e for e in group if e[1] == 1]
        outs = [e for e in group if e[1] == 0]
        want = 1 if last == 0 else 0
        while ins or outs:
            pick = (outs if want == 0 else ins) or (ins or outs)
            e = pick.pop(0)
            out.append(e)
            last = e[1]
            want = 1 - e[1]
        i = j
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("--proc", required=True, help="image name of the process whose audio threads are read")
    ap.add_argument("--min-us", type=float, default=100.0)
    ap.add_argument("--tids", default="")
    ap.add_argument("--rows", type=int, default=60)
    ap.add_argument("--chain", type=int, default=3,
                    help="steps of the holder chain under each long wait (0 = off)")
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

    cut.report_skipped()
    pids = cut.need_pids([a.proc])
    if pids is None:
        return 1
    audio = set()
    for tid, v in threads.items():
        if v[0] not in pids:
            continue
        pid = v[0]
        start = mod(pid, v[1])
        if cutlib.is_audio_thread(v[3], start, a.audio_module):
            audio.add(tid)
    pidtxt = ",".join(str(p) for p in pids)
    print(f"{a.proc} pid {pidtxt}: {len(audio)} audio threads")

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
    # Every thread's switches in their own order, and its readies, for the holder chain. Built on the first long
    # wait only: a real cut has ~1.7 M switch rows.
    seq: dict = {}
    seq_t: dict = {}
    all_rdy: dict = {}

    def build_holder_index() -> None:
        events = collections.defaultdict(list)
        for t, cpu, new, old, npri, opri, reason, state, wt in cs:
            events[new].append((t, 1, 0, 0))
            events[old].append((t, 0, state, reason))
        for tid, ev in events.items():
            seq[tid] = order_switches(ev)
            seq_t[tid] = [e[0] for e in seq[tid]]
        rdy_all = collections.defaultdict(list)
        for t, cpu, tid, reason, inc in rd:
            rdy_all[tid].append((t, cpu))
        all_rdy.update(rdy_all)

    def holder_chain(tid: int, at: float) -> list:
        """`tid` made a waiter ready at `at`: what it had waited on itself before it ran, and who made it ready,
        then the same for that thread, up to --chain steps. Stops at an idle or unknown waker, at a thread
        already in the chain, and at a holder that was not waiting (preempted, or running since before the cut)."""
        if not seq:
            build_holder_index()
        lines, seen = [], set()
        while len(lines) < a.chain and tid > 0 and tid not in seen:
            seen.add(tid)
            start = mod(threads[tid][0], threads[tid][1]) if tid in threads else "?"
            ev = seq.get(tid, [])
            i = bisect.bisect_right(seq_t.get(tid, []), at) - 1
            while i >= 0 and ev[i][1] != 1:
                i -= 1
            if i < 0:
                lines.append(f"{who(tid)} [start {start}] running since before the cut")
                break
            back = ev[i][0]
            if i == 0 or ev[i - 1][1] != 0 or ev[i - 1][2] != WAITING:
                state = "no switch-out before" if i == 0 else STATE.get(ev[i - 1][2], ev[i - 1][2])
                lines.append(f"{who(tid)} [start {start}] ran from {back:.3f} ms ({state}, not a wait)")
                break
            t0, _, _, reason = ev[i - 1]
            rr = all_rdy.get(tid, [])
            k = bisect.bisect_right(rr, (back, 1 << 30)) - 1
            by = running(rr[k][1], rr[k][0]) if k >= 0 and rr[k][0] >= t0 else -1
            lines.append(f"{who(tid)} [start {start}] had waited {WAIT.get(reason, reason)} "
                         f"{(back - t0) * 1000.0:.0f} us ({t0:.3f} .. {back:.3f} ms), made ready by "
                         f"{who(by) if by >= 0 else '?'}")
            if by <= 0:
                break
            tid, at = by, rr[k][0]
        return lines

    kinds = collections.Counter()
    found = []
    for t, cpu, new, old, npri, opri, reason, state, wt in cs:
        if old not in audio or state != WAITING or reason == USER_REQUEST:
            continue
        i = bisect.bisect_right(ins[old], t)
        back = ins[old][i] if i < len(ins[old]) else None
        dur = (back - t) * 1000.0 if back is not None else -1.0
        kinds[(WAIT.get(reason, reason), "out" if dur < 0 else "long" if dur >= a.min_us else "short")] += 1
        if dur >= a.min_us or dur < 0:
            j = bisect.bisect_right(rdy[old], (t, 99))
            r = rdy[old][j] if j < len(rdy[old]) else None
            waker = running(r[1], r[0]) if r else -1
            by = who(waker) if r else "?"
            s = smp[old]
            q = bisect.bisect_right(s, (t, 1 << 70)) - 1
            last = f"{mod(threads[old][0], s[q][1])} ({(t - s[q][0]):.2f} ms before)" if q >= 0 else "-"
            chain = holder_chain(waker, r[0]) if r and a.chain > 0 and dur >= 0 else []
            found.append((dur, t, who(old), WAIT.get(reason, reason), by, last, chain))
    print("switch-outs of audio threads that are not the wait for work, by reason (long = at least "
          f"{a.min_us:.0f} us):")
    labels = {"long": "long ", "short": "short", "out": "still out at the cut's end"}
    for (k, kind), n in sorted(kinds.items(), key=lambda x: -x[1]):
        print(f"  {k:20} {labels[kind]} x{n}")
    done = [f for f in found if f[0] >= 0]
    still = [f for f in found if f[0] < 0]
    print(f"\nthe long ones ({len(done)}), longest first:")
    for dur, t, w, k, by, last, chain in sorted(done, key=lambda f: (f[0], f[1]), reverse=True)[: a.rows]:
        print(f"  {dur:9.0f} us at {t:10.3f} ms  {w:28} {k:18} made ready by {by:34} last sample {last}")
        for line in chain:
            print(f"      holder: {line}")
    print(f"\nstill out at the cut's end ({len(still)}):")
    for dur, t, w, k, by, last, chain in sorted(still, key=lambda f: f[1])[: a.rows]:
        print(f"  out at {t:10.3f} ms  {w:28} {k:18} made ready by {by:34} last sample {last}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
