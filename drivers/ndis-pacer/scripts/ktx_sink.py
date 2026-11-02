#!/usr/bin/env python3
"""Receive the dasio kernel test stream (scripts/ktx.ps1) on Linux and report it.

Each frame's payload starts with its sequence number and its send time
(100 ns, the sender's interrupt time), both 8-byte big endian. Arrival times
are the kernel's receive timestamps (SO_TIMESTAMPNS), so the receiving host's
own scheduling does not show. The delay of a frame is its arrival minus its
send time, less the smallest delay seen (the clocks differ by a constant);
a stall below the sender's timer shows as delay, a lost frame as a missing
sequence. With the stream's interval given, the schedule delay is also
reported: arrival minus the frame's slot in the schedule (sequence x
interval), less its smallest; it shows a late timer fire on the sender too.

usage: ktx_sink.py <port> <seconds> [interval_us] [json-out]
"""
import json
import socket
import struct
import sys
import time

SO_TIMESTAMPNS = 35
SCM_TIMESTAMPNS = SO_TIMESTAMPNS
BIN_S = 5


def receive(port: int, seconds: float) -> list[tuple[int, int, int]]:
    """(sequence, send time in ns, arrival in ns) per frame, for `seconds`."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 8 << 20)
    s.setsockopt(socket.SOL_SOCKET, SO_TIMESTAMPNS, 1)
    s.bind(("0.0.0.0", port))
    s.settimeout(1.0)
    out: list[tuple[int, int, int]] = []
    first = None
    deadline = None
    while deadline is None or time.monotonic() < deadline:
        try:
            data, anc, _flags, _addr = s.recvmsg(2048, socket.CMSG_SPACE(16))
        except socket.timeout:
            continue
        stamp = None
        for level, kind, cdata in anc:
            if level == socket.SOL_SOCKET and kind == SCM_TIMESTAMPNS:
                sec, nsec = struct.unpack("qq", cdata[:16])
                stamp = sec * 1_000_000_000 + nsec
        if stamp is None or len(data) < 16:
            raise SystemExit("a frame without a kernel timestamp or payload")
        seq, sent = struct.unpack(">QQ", data[:16])
        if first is None:
            first = stamp
            deadline = time.monotonic() + seconds
        out.append((seq, sent * 100, stamp))
    return out


def pct(values: list[int], p: float) -> int:
    if not values:
        return 0
    v = sorted(values)
    return v[min(len(v) - 1, int(p * len(v)))]


def report(frames: list[tuple[int, int, int]], interval_us: int = 0) -> dict:
    if not frames:
        raise SystemExit("no frames received")
    base = min(a - s for _q, s, a in frames)
    t0 = frames[0][2]
    seqs = [q for q, _s, _a in frames]
    lost = (max(seqs) - min(seqs) + 1) - len(set(seqs))
    delays = [(a - s - base) // 1000 for _q, s, a in frames]
    bins = []
    for i in range(0, int((frames[-1][2] - t0) / 1e9 / BIN_S) + 1):
        lo, hi = t0 + i * BIN_S * 10**9, t0 + (i + 1) * BIN_S * 10**9
        part = [(q, s, a) for q, s, a in frames if lo <= a < hi]
        if not part:
            bins.append({"t": i * BIN_S, "frames": 0})
            continue
        gaps = [(part[j][2] - part[j - 1][2]) // 1000 for j in range(1, len(part))]
        d = [(a - s - base) // 1000 for _q, s, a in part]
        bins.append({"t": i * BIN_S, "frames": len(part), "max_gap_us": max(gaps, default=0),
                     "max_delay_us": max(d)})
    sched = {}
    if interval_us > 0:
        q0 = frames[0][0]
        late = [a - (q - q0) * interval_us * 1000 for q, _s, a in frames]
        lo = min(late)
        sd = [(x - lo) // 1000 for x in late]
        sched = {"schedule_delay_us": {"p50": pct(sd, 0.5), "p99": pct(sd, 0.99), "p999": pct(sd, 0.999),
                                       "max": max(sd)},
                 "schedule_over_1ms": sum(1 for d in sd if d > 1000),
                 "schedule_over_4ms": sum(1 for d in sd if d > 4000)}
    return {
        **sched,
        "frames": len(frames),
        "lost": lost,
        "delay_us": {"p50": pct(delays, 0.5), "p99": pct(delays, 0.99), "p999": pct(delays, 0.999),
                     "max": max(delays)},
        "frames_over_1ms": sum(1 for d in delays if d > 1000),
        "frames_over_4ms": sum(1 for d in delays if d > 4000),
        "bins": bins,
    }


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    interval = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    r = report(receive(int(sys.argv[1]), float(sys.argv[2])), interval)
    print(json.dumps({k: v for k, v in r.items() if k != "bins"}))
    worst = sorted((b for b in r["bins"] if b.get("frames")), key=lambda b: -b["max_delay_us"])[:8]
    for b in worst:
        print(f"t={b['t']:>4}s frames={b['frames']} max_gap={b['max_gap_us']}us max_delay={b['max_delay_us']}us")
    if len(sys.argv) > 4:
        with open(sys.argv[4], "w") as f:
            json.dump(r, f)


if __name__ == "__main__":
    main()
