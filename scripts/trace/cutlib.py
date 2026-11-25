"""dasio#207: shared reader of the cut files `etl-cut.ps1` writes (rows C/R/S/I/D/T/P/M/K).

The csw_*.py tools all start from `load()`. Row formats are documented in the header of etl-cut.ps1.
Times in a cut are 100 ns steps from the cut's -AtUtc; `load()` turns them into milliseconds.
"""
import bisect
import collections

KERNEL = 0xFFFF000000000000

# KWAIT_REASON and KTHREAD_STATE values as they appear in C rows.
WAIT = {0: "Executive", 4: "DelayExecution", 5: "Suspended", 6: "UserRequest", 7: "WrExecutive", 13: "WrUserRequest",
        15: "WrQueue", 16: "WrLpcReceive", 17: "WrLpcReply", 21: "WrKeyedEvent", 27: "WrResource", 28: "WrPushLock",
        29: "WrMutex", 30: "WrQuantumEnd", 31: "WrDispatchInt", 32: "WrPreempted", 33: "WrYield", 34: "WrFastMutex",
        35: "WrGuardedMutex", 37: "WrAlertByThreadId", 38: "WrDeferredPreempt"}
STATE = {0: "Init", 1: "Ready", 2: "Running", 3: "Standby", 4: "Terminated", 5: "Waiting", 6: "Transition", 7: "DeferredReady"}
WR_RESOURCE = 27
WAITING = 5
USER_REQUEST = 6


def parse_tags(items):
    """`--tag NAME=image.exe` (repeatable) -> {image lower case: NAME}. No tag, no renaming."""
    tags = {}
    for item in items or []:
        name, sep, image = item.partition("=")
        if not sep or not name or not image:
            raise SystemExit(f"--tag wants NAME=image.exe, got {item!r}")
        tags[image.lower()] = name
    return tags


def add_tag_option(ap):
    ap.add_argument("--tag", action="append", default=[], metavar="NAME=image.exe",
                    help="show this image under a short NAME (repeatable); default: the image name")


def add_audio_option(ap):
    ap.add_argument("--audio-module", action="append", default=[], metavar="TEXT",
                    help="also treat a thread started in a module whose name contains TEXT as an audio thread "
                         "(repeatable; 'asio' always counts, so do threads named AudioCalc or dasio)")


def is_audio_thread(name, start, extra=()):
    """True for a thread named AudioCalc/dasio, or started in a module named like an ASIO driver (or `extra`)."""
    start = start.lower()
    return name in ("AudioCalc", "dasio") or "asio" in start or any(x.lower() in start for x in extra)


class Cut:
    """One loaded cut: the process/thread/module tables and the event rows, times in ms."""

    def __init__(self, tags=None):
        self.tags = tags or {}
        self.threads = {}   # tid -> (pid, start address, base priority, name)
        self.procs = {}     # pid -> image
        self.mods = collections.defaultdict(list)   # pid -> [(base, end, file, in system32)]
        self.kern = []      # the modules at kernel addresses, sorted
        self.cs, self.rd, self.sm, self.isr, self.dpc, self.ks = [], [], [], [], [], []
        self.last_on = {}   # cpu -> the thread switched in last before the window

    def mod(self, pid, addr):
        """`file+offset` of the module holding `addr`, else the bare address."""
        lst = self.kern if addr >= KERNEL else self.mods.get(pid, [])
        i = bisect.bisect_right(lst, (addr, 1 << 70, "", True)) - 1
        if i >= 0 and lst[i][0] <= addr < lst[i][1]:
            return f"{lst[i][2]}+{addr - lst[i][0]:x}"
        return f"{addr:x}"

    def who(self, tid):
        """`image:tid(name)` of a thread, the image shown under its --tag name when given."""
        if tid == 0:
            return "idle"
        t = self.threads.get(tid)
        if not t:
            return f"tid {tid}"
        img = self.procs.get(t[0], f"pid {t[0]}")
        return f"{self.tags.get(img.lower(), img)}:{tid}" + (f"({t[3]})" if t[3] else "")

    def label(self, pid):
        img = self.procs.get(pid, str(pid))
        return self.tags.get(img.lower(), img)

    def pids(self, images):
        """pid -> image for every process whose image is one of `images` (case ignored)."""
        want = {i.lower() for i in images}
        return {p: n for p, n in self.procs.items() if n.lower() in want}


def load(path, kinds="CRSIDTPMK", window=None, tags=None):
    """Read a cut. `kinds` picks the row letters to keep; `window` = (from_ms, to_ms) drops C/R/S/I/D rows outside it
    (the thread running on each CPU before the window is kept in `last_on`)."""
    cut = Cut(tags)
    lo, hi = window if window else (None, None)
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            k = line[:1]
            if not k or k not in kinds:
                continue
            p = line.rstrip("\n").split(",")
            if k in "CRSIDK":
                t = int(p[1]) / 1e4
                if lo is not None and k != "K" and not lo <= t <= hi:
                    if k == "C" and t < lo:
                        cut.last_on[int(p[2])] = int(p[3])
                    continue
            if k == "C":
                cut.cs.append((t, int(p[2]), int(p[3]), int(p[4]), int(p[5]), int(p[6]), int(p[7]), int(p[8]), int(p[9])))
            elif k == "R":
                cut.rd.append((t, int(p[2]), int(p[3]), int(p[6]), int(p[7])))
            elif k == "S":
                cut.sm.append((t, int(p[2]), int(p[3]), int(p[4], 16)))
            elif k == "I":
                cut.isr.append((t, int(p[2]), int(p[3], 16)))
            elif k == "D":
                cut.dpc.append((t, int(p[2]), int(p[3], 16)))
            elif k == "K":
                cut.ks.append((t, int(p[3]), int(p[4]), int(p[5]), [int(x, 16) for x in p[6].split(";") if x]))
            elif k == "T":
                cut.threads[int(p[3])] = (int(p[2]), int(p[4], 16), int(p[5]), p[6] if len(p) > 6 else "")
            elif k == "P":
                cut.procs[int(p[2])] = p[4]
            elif k == "M":
                base = int(p[3], 16)
                name = p[5]
                cut.mods[int(p[2])].append((base, base + int(p[4], 16), name.split("\\")[-1].lower(),
                                            "\\windows\\system32\\" in name.lower()))
    for v in cut.mods.values():
        v.sort()
    cut.kern = sorted(m for v in cut.mods.values() for m in v if m[0] >= KERNEL)
    for rows in (cut.cs, cut.rd, cut.sm, cut.isr, cut.dpc):
        rows.sort()
    return cut
