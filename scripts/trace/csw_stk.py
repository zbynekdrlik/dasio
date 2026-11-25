#!/usr/bin/env python3
"""dasio#207: the stacks of audio threads that waited on a kernel resource, from a trace cut with K rows.

    csw_stk.py <cut file> --proc a.exe [--proc b.exe ...] [--tag NAME=image.exe] [--all-threads] [--top 12]

K rows (etl-cut.ps1) are the stacks of threads switched in after a WrResource wait: the kernel part and the user
part share one stamp. Groups them per process and call chain; system dlls get their export names (nearest export
at or below the address: exact for win32u.dll's syscall stubs, approximate elsewhere; read from
C:\\Windows\\System32 on the PC that runs this, no names when the dll is not there).
"""
import argparse
import bisect
import collections
import os
import struct
import sys

import cutlib
from cutlib import KERNEL, WAITING, WR_RESOURCE

_exports = {}


def exports(name: str):
    if name in _exports:
        return _exports[name]
    out = []
    try:
        with open(os.path.join(r"C:\Windows\System32", name), "rb") as fh:
            d = fh.read()
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        optsz = struct.unpack_from("<H", d, pe + 20)[0]
        opt = pe + 24
        dd = opt + (112 if struct.unpack_from("<H", d, opt)[0] == 0x20B else 96)
        exp_rva = struct.unpack_from("<I", d, dd)[0]
        secs = []
        for i in range(nsec):
            vs, va, rs, ro = struct.unpack_from("<IIII", d, opt + optsz + i * 40 + 8)
            secs.append((va, max(vs, rs), ro))

        def off(rva):
            for va, sz, ro in secs:
                if va <= rva < va + sz:
                    return rva - va + ro
            raise ValueError(rva)
        e = off(exp_rva)
        nnames = struct.unpack_from("<I", d, e + 24)[0]
        afun, anames, aord = struct.unpack_from("<III", d, e + 28)
        for i in range(nnames):
            nrva = struct.unpack_from("<I", d, off(anames) + 4 * i)[0]
            o = struct.unpack_from("<H", d, off(aord) + 2 * i)[0]
            frva = struct.unpack_from("<I", d, off(afun) + 4 * o)[0]
            no = off(nrva)
            out.append((frva, d[no:d.index(b"\0", no)].decode("ascii", "replace")))
        out.sort()
    except (OSError, ValueError, struct.error):
        out = []
    _exports[name] = out
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cut")
    ap.add_argument("--proc", action="append", required=True, help="image name of a process to read (repeatable)")
    ap.add_argument("--all-threads", action="store_true", help="every thread of the processes, not only audio threads")
    ap.add_argument("--top", type=int, default=12)
    ap.add_argument("--frames", type=int, default=14)
    cutlib.add_tag_option(ap)
    cutlib.add_audio_option(ap)
    a = ap.parse_args(argv)
    cut = cutlib.load(a.cut, kinds="CTPMK", tags=cutlib.parse_tags(a.tag))
    threads, ks = cut.threads, cut.ks
    outs = collections.defaultdict(list)   # tid -> [t of a WrResource switch-out]
    ins = collections.defaultdict(list)
    for t, cpu, new, old, npri, opri, reason, state, wt in cut.cs:
        ins[new].append(t)
        if reason == WR_RESOURCE and state == WAITING:
            outs[old].append(t)

    def frame(pid: int, addr: int) -> str:
        lst = cut.kern if addr >= KERNEL else cut.mods.get(pid, [])
        i = bisect.bisect_right(lst, (addr, 1 << 70, "", True)) - 1
        if i >= 0 and lst[i][0] <= addr < lst[i][1]:
            name, rva = lst[i][2], addr - lst[i][0]
            if lst[i][3] and addr < KERNEL:
                ex = exports(name)
                j = bisect.bisect_right(ex, (rva, "\uffff")) - 1
                if j >= 0 and rva - ex[j][0] < 0x400:
                    return f"{name}!{ex[j][1]}+{rva - ex[j][0]:x}"
            return f"{name}+{rva:x}"
        return f"{addr:x}"

    cut.report_skipped()
    found = cut.need_pids(a.proc)
    if found is None:
        return 1
    two = {p: cut.label(p) for p in found}
    audio = set()
    for tid, v in threads.items():
        if v[0] in two:
            start = frame(v[0], v[1])
            if a.all_threads or cutlib.is_audio_thread(v[3], start, a.audio_module):
                audio.add(tid)
    print(f"K rows {len(ks)}; threads looked at {len(audio)} ({'all of the processes' if a.all_threads else 'audio threads of the processes'})")

    merged = collections.OrderedDict()
    for t, pid, tid, stamp, fr in ks:
        if tid not in audio:
            continue
        m = merged.setdefault((tid, stamp), [t, pid, [], []])
        for x in fr:
            (m[2] if x >= KERNEL else m[3]).append(x)
    print(f"stacks of those threads after a WrResource wait: {len(merged)}")

    def waited(tid: int, t: float) -> float:
        o = outs.get(tid, [])
        i = bisect.bisect_right(o, t) - 1
        if i < 0:
            return -1.0
        j = bisect.bisect_right(ins.get(tid, []), o[i])
        return (ins[tid][j] - o[i]) * 1000.0 if j < len(ins.get(tid, [])) else -1.0

    groups = collections.defaultdict(list)
    for (tid, stamp), (t, pid, kf, uf) in merged.items():
        kmods = []
        for x in kf:
            m = frame(pid, x).split("+")[0]
            if not kmods or kmods[-1] != m:
                kmods.append(m)
        ufr = [frame(pid, x) for x in uf[: a.frames]]
        tag = two.get(pid, str(pid))
        tname = threads.get(tid, (0, 0, 0, ""))[3] or "asio"
        groups[(tag, tname, " > ".join(kmods[:8]), " | ".join(ufr[:6]))].append((waited(tid, t), t, tid, ufr))
    for key, lst in sorted(groups.items(), key=lambda kv: -len(kv[1]))[: a.top]:
        durs = sorted((d for d, _, _, _ in lst), reverse=True)
        print(f"\nx{len(lst)}  {key[0]} {key[1]}  waited us: max {durs[0]:.0f}, median {durs[len(durs) // 2]:.0f}, total {sum(max(d, 0) for d in durs):.0f}")
        print(f"   kernel: {key[2]}")
        ex = max(lst)
        print(f"   user (the longest, {ex[0]:.0f} us at {ex[1]:.3f} ms, tid {ex[2]}):")
        for fr in ex[3]:
            print(f"      {fr}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
