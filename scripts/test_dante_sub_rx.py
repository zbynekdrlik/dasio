#!/usr/bin/env python3
"""Tests of scripts/dante_sub_rx.py (dasio#207). netaudio is not needed."""

import pathlib
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import dante_sub_rx as sub  # noqa: E402


def result(**kw):
    base = dict(unchanged={}, verified={}, failures={}, sent={}, unconfirmed={})
    base.update(kw)
    return SimpleNamespace(**base)


class ParsePairs(unittest.TestCase):
    def test_a_subscription_and_a_clear(self):
        self.assertEqual(
            sub.parse_pairs(["1=free 01@Card", "2=-"]),
            {1: ("free 01", "Card"), 2: None},
        )

    def test_the_last_at_splits_the_device(self):
        self.assertEqual(sub.parse_pairs(["3=a@b@dev"]), {3: ("a@b", "dev")})

    def test_bad_input_is_a_clear_error(self):
        for bad, words in [
            ([], "no <rxnum>"),
            (["1"], "expected <rxnum>"),
            (["x=a@b"], "not a whole number"),
            (["0=a@b"], "count from 1"),
            (["-1=a@b"], "count from 1"),
            (["1=ab"], "expected <txname>@<txdevice>"),
            (["1=@dev"], "expected <txname>@<txdevice>"),
            (["1=name@"], "expected <txname>@<txdevice>"),
            (["1=a@b", "1=-"], "given twice"),
        ]:
            with self.subTest(bad=bad):
                with self.assertRaises(sub.UsageError) as ctx:
                    sub.parse_pairs(bad)
                self.assertIn(words, str(ctx.exception))


class Describe(unittest.TestCase):
    def test_verified_is_success_and_printed(self):
        lines, code = sub.describe(result(verified={1: ("a", "b")}), {1: ("a", "b")})
        self.assertEqual(code, 0)
        self.assertIn("verified: {1: ('a', 'b')}", lines)

    def test_unchanged_and_sent_are_success(self):
        self.assertEqual(sub.describe(result(unchanged={1: None}), {1: None})[1], 0)
        self.assertEqual(sub.describe(result(sent={1: ("a", "b")}), {1: ("a", "b")})[1], 0)

    def test_a_failure_or_unconfirmed_is_exit_1(self):
        self.assertEqual(sub.describe(result(failures={1: "no"}), {1: None})[1], 1)
        self.assertEqual(sub.describe(result(unconfirmed={1: "no"}), {1: None})[1], 1)

    def test_a_wanted_channel_the_result_omits_is_exit_1(self):
        lines, code = sub.describe(result(verified={1: None}), {1: None, 2: None})
        self.assertEqual(code, 1)
        self.assertIn("not in the result: [2]", lines)


if __name__ == "__main__":
    unittest.main()
