#!/usr/bin/env python3
"""Tests of scripts/mdns_watch.py (dasio#167)."""

import pathlib
import struct
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import mdns_watch  # noqa: E402

ARC = b"\x00\x00\x84\x00" + b"\x09test-dev\x0c_netaudio-arc\x04_udp\x05local\x00"


class Classify(unittest.TestCase):
    def setUp(self):
        self.control = mdns_watch.control_query("ab12")

    def test_our_control_query_is_recognised_by_its_bytes(self):
        self.assertEqual(mdns_watch.classify("10.1.0.4", self.control, self.control), "control")
        self.assertIsNone(mdns_watch.classify("10.1.0.4", mdns_watch.control_query("cd34"), self.control))

    def test_dante_mdns_is_split_by_the_interface_it_arrived_on(self):
        self.assertEqual(mdns_watch.classify("127.0.0.1", ARC, self.control), "dante loopback")
        self.assertEqual(mdns_watch.classify("127.0.0.2", ARC, self.control), "dante loopback")

    def test_the_interface_decides_not_the_source(self):
        # A datagram from 127.0.0.1 that arrived on the routed interface
        # is on the network: classify gets the arrival interface.
        self.assertEqual(mdns_watch.classify("10.1.0.4", ARC, self.control), "dante network")
        self.assertEqual(mdns_watch.classify("10.1.0.4", ARC, self.control), "dante network")

    def test_other_mdns_is_not_counted(self):
        other = b"\x00\x00\x00\x00" + b"\x05_http\x04_tcp\x05local\x00"
        self.assertIsNone(mdns_watch.classify("10.1.0.4", other, self.control))
        self.assertIsNone(mdns_watch.classify("127.0.0.1", other, self.control))


class ControlQuery(unittest.TestCase):
    def test_one_ptr_question_naming_the_nonce(self):
        q = mdns_watch.control_query("ab12")
        self.assertEqual(struct.unpack("!HHHHHH", q[:12]), (0, 0, 1, 0, 0, 0))
        self.assertEqual(q[12:], b"\x04ab12\x10dasio-mdns-watch\x05local\x00\x00\x0c\x00\x01")
        self.assertNotIn(mdns_watch.DANTE, q)


class Verdict(unittest.TestCase):
    def test_clean_run(self):
        self.assertEqual(mdns_watch.verdict({"dante loopback": 40}, set(), True, True), [])
        self.assertEqual(mdns_watch.verdict({}, set(), True, False), [])

    def test_a_device_on_the_network_fails_it(self):
        p = mdns_watch.verdict({"dante loopback": 3, "dante network": 7}, {"10.1.0.4"}, True, True)
        self.assertEqual(
            p,
            [
                "7 Dante mDNS datagrams on the network (sources ['10.1.0.4']): "
                "a test device is not on loopback"
            ],
        )

    def test_a_deaf_listener_fails_it(self):
        p = mdns_watch.verdict({"dante loopback": 3}, set(), False, True)
        self.assertEqual(len(p), 1)
        self.assertTrue(p[0].startswith("our own control query never came back"), p[0])

    def test_no_loopback_announcement_fails_only_when_required(self):
        p = mdns_watch.verdict({}, set(), True, True)
        self.assertEqual(len(p), 1)
        self.assertTrue(p[0].startswith("no Dante mDNS on 127.0.0.1"), p[0])
        self.assertEqual(mdns_watch.verdict({"dante loopback": 0}, set(), True, False), [])

    def test_every_problem_is_named(self):
        p = mdns_watch.verdict({"dante network": 1}, {"10.1.0.4"}, False, True)
        self.assertEqual(len(p), 3)


if __name__ == "__main__":
    unittest.main()
