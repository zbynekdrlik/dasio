#!/usr/bin/env python3
"""Tests of scripts/integrity.py (dasio#28, E14)."""

import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import integrity  # noqa: E402


def tree(files):
    """A temporary repo root holding `files` ({relative path: text})."""
    d = tempfile.TemporaryDirectory()
    for rel, text in files.items():
        p = pathlib.Path(d.name, rel)
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
    return d


class Integrity(unittest.TestCase):
    def test_realtime_outside_the_audited_places_is_a_problem(self):
        line = "unsafe { SetPriorityClass(GetCurrentProcess(), REALTIME_PRIORITY_CLASS) };\n"
        with tree({"crates/dasio/src/other.rs": line, "crates/dasio/src/run.rs": line}) as d:
            problems = integrity.scan(d)
        self.assertEqual(len(problems), 1)
        self.assertTrue(problems[0].startswith("crates/dasio/src/other.rs:1: REALTIME"))

    def test_reading_the_class_is_not_setting_it(self):
        text = "REALTIME_PRIORITY_CLASS => \"realtime\",\n"
        with tree({"crates/dasio/src/pc_profile/windows.rs": text}) as d:
            self.assertEqual(integrity.scan(d), [])

    def test_a_socket_on_a_ptp_port_is_a_problem(self):
        text = 'let s = UdpSocket::bind("0.0.0.0:319")?;\nlet x = 320; // a count\n'
        with tree({"crates/dasio/src/clock.rs": text}) as d:
            problems = integrity.scan(d)
        self.assertEqual(problems, ["crates/dasio/src/clock.rs:1: the PTP ports 319/320 belong to dsync/dstack"])

    def test_the_exit_code_follows_the_problems(self):
        with tree({"crates/a/src/lib.rs": "fn main() {}\n"}) as d:
            self.assertEqual(integrity.main(["integrity.py", d]), 0)
        bad = "SetPriorityClass(h, REALTIME_PRIORITY_CLASS);\n"
        with tree({"crates/a/src/lib.rs": bad}) as d:
            self.assertEqual(integrity.main(["integrity.py", d]), 1)

    def test_this_repo_is_clean(self):
        root = pathlib.Path(__file__).resolve().parent.parent
        self.assertEqual(integrity.scan(root), [])


if __name__ == "__main__":
    unittest.main()
