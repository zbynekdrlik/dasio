#!/usr/bin/env python3
"""dasio's integrity scan (dasio#28, E14): rules the compiler does not check.

- The process priority class goes to REALTIME only in audited places
  (the device's run.rs and the jitter probe).
- No dasio crate opens the PTP ports 319/320: dsync and dstack own them.

Prints each violation as `file:line: rule` and exits 1 when there is one.
Usage: integrity.py [repo root]
"""

import pathlib
import re
import sys

REALTIME_ALLOWED = {
    "crates/dasio/src/run.rs",
    "crates/dasio/src/jitter.rs",
}
SET_REALTIME = re.compile(r"SetPriorityClass\(.*REALTIME_PRIORITY_CLASS")
PTP_PORT = re.compile(r"\b(319|320)\b")
SOCKET_WORD = re.compile(r"\b(bind|UdpSocket|SocketAddr\w*)\b")


def scan(root):
    """Every violation under `root`/crates, as `file:line: rule` strings."""
    root = pathlib.Path(root)
    out = []
    for path in sorted((root / "crates").rglob("*.rs")):
        rel = path.relative_to(root).as_posix()
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if SET_REALTIME.search(line) and rel not in REALTIME_ALLOWED:
                out.append(
                    f"{rel}:{n}: REALTIME_PRIORITY_CLASS is set only in "
                    f"{', '.join(sorted(REALTIME_ALLOWED))}"
                )
            if PTP_PORT.search(line) and SOCKET_WORD.search(line):
                out.append(f"{rel}:{n}: the PTP ports 319/320 belong to dsync/dstack")
    return out


def main(argv):
    root = argv[1] if len(argv) > 1 else "."
    problems = scan(root)
    for p in problems:
        print(p)
    print(f"integrity: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
