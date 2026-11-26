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


class Retries(unittest.TestCase):
    """mDNS discovery misses the RX device now and then (alex-klavir 26.11.2026: `RX device alex-klavir not
    found` on 6 of ~14 calls, each found again a few seconds later). A missed device is tried again, a bounded
    number of times; a device that was found ends the loop with that attempt's own exit code."""

    def attempts(self, outcomes):
        calls = []

        def attempt():
            calls.append(1)
            return outcomes[len(calls) - 1]

        return calls, attempt

    def test_a_device_found_at_once_runs_one_attempt(self):
        calls, attempt = self.attempts([(0, True)])
        self.assertEqual(sub.with_retries(attempt, log=lambda m: None), 0)
        self.assertEqual(len(calls), 1)

    def test_a_missed_device_is_tried_again_and_its_found_attempt_decides(self):
        calls, attempt = self.attempts([(1, False), (1, False), (1, True)])
        self.assertEqual(sub.with_retries(attempt, log=lambda m: None), 1)
        self.assertEqual(len(calls), 3)
        calls, attempt = self.attempts([(1, False), (0, True)])
        self.assertEqual(sub.with_retries(attempt, log=lambda m: None), 0)
        self.assertEqual(len(calls), 2)

    def test_a_device_never_found_stops_after_the_tries_with_exit_1(self):
        said = []
        calls, attempt = self.attempts([(0, False)] * 10)
        self.assertEqual(sub.with_retries(attempt, tries=4, log=said.append), 1)
        self.assertEqual(len(calls), 4)
        self.assertIn("not found in 4 tries", said[-1])

    def test_the_default_tries_more_than_once(self):
        self.assertGreater(sub.FIND_TRIES, 1)

    def test_a_missing_device_marks_the_attempt_or_ends_the_program_without_a_loop(self):
        seen = {"found": True}
        self.assertEqual(sub.device_missing("alex-klavir", seen), 1)
        self.assertFalse(seen["found"])
        with self.assertRaises(SystemExit) as ctx:
            sub.device_missing("alex-klavir", None)
        self.assertIn("RX device alex-klavir not found", str(ctx.exception))

    def test_each_attempt_starts_found_so_a_miss_never_carries_over(self):
        seen = {"found": True}
        results = iter([None, 0, 1])

        def call():
            rc = next(results)
            if rc is None:  # the first call misses the device
                sub.device_missing("x", seen)
                return 1
            return rc

        attempt = sub.make_attempt(call, seen)
        self.assertEqual(attempt(), (1, False))
        self.assertEqual(attempt(), (0, True))
        self.assertEqual(attempt(), (1, True))

    def test_an_attempt_with_no_exit_code_is_exit_0(self):
        attempt = sub.make_attempt(lambda: None, {"found": True})
        self.assertEqual(attempt(), (0, True))

    def test_the_loop_retries_a_real_attempt_until_found(self):
        seen = {"found": True}
        misses = iter([True, True, False])

        def call():
            if next(misses):
                return sub.device_missing("x", seen)
            return 0

        self.assertEqual(sub.with_retries(sub.make_attempt(call, seen), log=lambda m: None), 0)


if __name__ == "__main__":
    unittest.main()
