#!/usr/bin/env python3
"""Runs a command while counting Dante mDNS on this box's network (dasio#167).

The test suite's devices must serve on loopback only (DSTACK_TEST_LOOPBACK,
`.cargo/config.toml`), so no Dante mDNS of theirs may reach the box's
network. This listens on 224.0.0.251:5353, joined on the box's routed IPv4
address and on 127.0.0.1, while the command runs, and counts the datagrams
that name a Dante service (`_netaudio-*`) by source: one from any address
but loopback is a test device on the network, and fails the run.

Two checks keep the count honest (a listener that hears nothing proves
nothing):
- before the command, a query of our own sent out on the routed address
  must come back to the listener (the box loops its own multicast back);
- with --require-loopback, the devices' own announcements on 127.0.0.1 must
  be heard (the e2e test's devices ran and the listener hears Dante mDNS).

Usage: mdns_watch.py [--require-loopback] -- <command> [args...]
The exit code is the command's when it failed, else 1 when a check failed,
else 0.
"""

import argparse
import ipaddress
import os
import socket
import struct
import subprocess
import sys
import threading
import time

GROUP = "224.0.0.251"
PORT = 5353
# Dante's mDNS service types: _netaudio-arc, -cmc, -dbc, -chan.
DANTE = b"_netaudio"
# How long the control query may take to come back.
CONTROL_WAIT_S = 5.0
# Late datagrams of the command's last device (its goodbye) still counted.
AFTER_S = 2.0


def routed_ipv4():
    """The box's IPv4 address on its default route (connecting a UDP
    socket picks the route and sends nothing)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("192.0.2.1", 9))
        ip = s.getsockname()[0]
    finally:
        s.close()
    if ipaddress.ip_address(ip).is_loopback or ip == "0.0.0.0":
        raise SystemExit(f"mdns_watch: no routed IPv4 address on this box: {ip}")
    return ip


def control_query(nonce):
    """A one-question mDNS query for `<nonce>.dasio-mdns-watch.local` (PTR):
    our own datagram, recognisable by its name."""
    header = struct.pack("!HHHHHH", 0, 0, 1, 0, 0, 0)
    name = b"".join(
        bytes([len(label)]) + label
        for label in (nonce.encode(), b"dasio-mdns-watch", b"local")
    )
    return header + name + b"\x00" + struct.pack("!HH", 12, 1)


def classify(src, data, control):
    """What a datagram from `src` is: "control" (our own query), "dante
    loopback", "dante network", or None (any other mDNS)."""
    if data == control:
        return "control"
    if DANTE not in data:
        return None
    if ipaddress.ip_address(src).is_loopback:
        return "dante loopback"
    return "dante network"


def verdict(counts, network_sources, control_seen, require_loopback):
    """The problems a run's counts show (empty: clean)."""
    problems = []
    if not control_seen:
        problems.append(
            "our own control query never came back: the listener hears nothing "
            "on the routed address, so its zero would prove nothing"
        )
    if counts.get("dante network", 0):
        problems.append(
            f"{counts['dante network']} Dante mDNS datagrams on the network "
            f"(sources {sorted(network_sources)}): a test device is not on loopback"
        )
    if require_loopback and not counts.get("dante loopback", 0):
        problems.append(
            "no Dante mDNS on 127.0.0.1: the test devices never announced, "
            "or the listener does not hear them"
        )
    return problems


def join(sock, iface):
    mreq = socket.inet_aton(GROUP) + socket.inet_aton(iface)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)


def listener(routed):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("", PORT))
    join(sock, routed)
    join(sock, "127.0.0.1")
    sock.settimeout(0.2)
    return sock


class Counter:
    def __init__(self, sock, control):
        self.sock = sock
        self.control = control
        self.counts = {}
        self.network_sources = set()
        self.control_seen = threading.Event()
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        while not self.stop.is_set():
            try:
                data, (src, _port) = self.sock.recvfrom(9000)
            except socket.timeout:
                continue
            kind = classify(src, data, self.control)
            if kind is None:
                continue
            if kind == "control":
                self.control_seen.set()
                continue
            self.counts[kind] = self.counts.get(kind, 0) + 1
            if kind == "dante network":
                self.network_sources.add(src)


def main(argv):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--require-loopback", action="store_true")
    p.add_argument("command", nargs=argparse.REMAINDER)
    args = p.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        p.error("give the command after --")

    routed = routed_ipv4()
    control = control_query(os.urandom(6).hex())
    counter = Counter(listener(routed), control)
    counter.thread.start()

    sender = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    sender.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(routed))
    sender.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)
    sender.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
    sender.sendto(control, (GROUP, PORT))
    sender.close()
    counter.control_seen.wait(CONTROL_WAIT_S)
    print(
        f"mdns_watch: listening on {GROUP}:{PORT} (joined on {routed} and 127.0.0.1); "
        f"control query back: {counter.control_seen.is_set()}",
        flush=True,
    )

    code = subprocess.call(command)
    time.sleep(AFTER_S)
    counter.stop.set()
    counter.thread.join()
    counter.sock.close()

    print(
        f"mdns_watch: Dante mDNS during the run: "
        f"{counter.counts.get('dante loopback', 0)} on 127.0.0.1, "
        f"{counter.counts.get('dante network', 0)} on the network "
        f"(sources {sorted(counter.network_sources)})",
        flush=True,
    )
    if code != 0:
        print(f"mdns_watch: the command failed with exit code {code}", file=sys.stderr)
        return code
    problems = verdict(
        counter.counts,
        counter.network_sources,
        counter.control_seen.is_set(),
        args.require_loopback,
    )
    for problem in problems:
        print(f"mdns_watch: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
