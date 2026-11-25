#!/usr/bin/env python3
"""Tests of the csw_*.py trace readers (dasio#207) against a small synthetic cut, fixtures/mini.cut.

The cut has one process (daw.exe, pid 100) with an audio thread (tid 1000, started in a card's ASIO dll, one
switch-in per 1.5 ms on CPU 8), a worker named AudioCalc (tid 1001), and a GUI thread of other.exe (tid 2000).
The audio thread waits on a kernel resource from 45.4 ms to 54.0 ms (a gap of 9 ms), readied at 53.9 ms while
other.exe:2000 runs on CPU 9. The worker waits 0.8 ms on a kernel resource, readied by the same thread.
"""
import contextlib
import io
import pathlib
import re
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import csw_an  # noqa: E402
import csw_dump  # noqa: E402
import csw_res  # noqa: E402
import csw_smp  # noqa: E402
import csw_stk  # noqa: E402
import cutlib  # noqa: E402

CUT = str(HERE / "fixtures" / "mini.cut")


def run(tool, *args):
    """Run a tool's main() on the fixture; return (exit code, stdout)."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = tool.main([*args])
    return code, out.getvalue()


class Analyse(unittest.TestCase):
    def test_finds_the_gap_and_the_audio_thread_given_tid(self):
        code, out = run(csw_an, CUT, "--proc", "daw.exe", "--tid", "1000")
        self.assertEqual(code, 0)
        self.assertIn("audio thread taken: daw.exe:1000", out)
        self.assertIn("gaps >= 3.0 ms: 1", out)
        self.assertRegex(out, r"9\.00 ms\s+from\s+45\.000 to\s+54\.000")
        self.assertIn("audio thread out at 45.400 on cpu8: Waiting/WrResource", out)
        self.assertIn("audio thread readied at 53.900 on cpu9 by other.exe:2000(gui)", out)

    def test_picks_the_most_regular_thread_without_tid(self):
        code, out = run(csw_an, CUT, "--proc", "daw.exe")
        self.assertEqual(code, 0)
        self.assertIn("audio thread taken: daw.exe:1000", out)

    def test_a_gap_limit_above_the_gap_lists_none(self):
        code, out = run(csw_an, CUT, "--proc", "daw.exe", "--tid", "1000", "--gap", "10")
        self.assertEqual(code, 0)
        self.assertIn("gaps >= 10.0 ms: 0", out)

    def test_unknown_process_is_an_error(self):
        code, out = run(csw_an, CUT, "--proc", "nothere.exe")
        self.assertEqual(code, 1)
        self.assertIn("no process nothere.exe in the cut", out)

    def test_tag_renames_the_image(self):
        _, out = run(csw_an, CUT, "--proc", "daw.exe", "--tid", "1000", "--tag", "MASTER=daw.exe", "--tag", "GUI=other.exe")
        self.assertIn("audio thread taken: MASTER:1000", out)
        self.assertIn("by GUI:2000(gui)", out)
        self.assertNotIn("daw.exe:1000", out)


class Resources(unittest.TestCase):
    def test_lists_a_wrresource_wait_with_duration_and_waker(self):
        code, out = run(csw_res, CUT, "--proc", "daw.exe")
        self.assertEqual(code, 0)
        worker = next(line for line in out.splitlines() if "AudioCalc" in line and "WrResource" in line)
        self.assertRegex(worker, r"\b800 us at\s+20\.000 ms")
        self.assertIn("made ready by other.exe:2000(gui)", worker)
        audio = next(line for line in out.splitlines() if "8600 us" in line)
        self.assertIn("45.400 ms", audio)
        self.assertIn("last sample card_asio.dll+2000 (0.20 ms before)", audio)

    def test_longest_wait_comes_first(self):
        _, out = run(csw_res, CUT, "--proc", "daw.exe")
        self.assertLess(out.index("8600 us"), out.index("800 us"))

    def test_waits_for_work_are_not_listed(self):
        # 60 switch-outs with UserRequest, only the two WrResource waits are listed
        _, out = run(csw_res, CUT, "--proc", "daw.exe")
        self.assertIn("the long ones (2)", out)
        self.assertNotIn("UserRequest", out)

    def test_min_us_hides_the_short_wait(self):
        _, out = run(csw_res, CUT, "--proc", "daw.exe", "--min-us", "1000")
        self.assertIn("the long ones (1)", out)
        self.assertNotIn(" 800 us", out)


    def test_audio_module_option_adds_a_thread_start_to_look_at(self):
        # tid 1002 (MainThread) starts in an address no module holds; without the option it is not an audio thread
        _, out = run(csw_res, CUT, "--proc", "daw.exe")
        self.assertIn("daw.exe pid 100: 2 audio threads", out)
        _, out = run(csw_res, CUT, "--proc", "daw.exe", "--audio-module", "30005000")
        self.assertIn("daw.exe pid 100: 3 audio threads", out)


class Stacks(unittest.TestCase):
    def test_groups_a_kernel_and_user_stack_by_stamp(self):
        code, out = run(csw_stk, CUT, "--proc", "daw.exe")
        self.assertEqual(code, 0)
        # K rows 5 (two parts for each of two stamps of daw.exe, one of other.exe); two stacks after merging by stamp
        self.assertIn("K rows 5", out)
        self.assertIn("stacks of those threads after a WrResource wait: 2", out)
        block = out[out.index("x1  daw.exe asio"):]
        self.assertIn("waited us: max 8600", block)
        self.assertIn("kernel: ntoskrnl.exe > card.sys", block)
        self.assertRegex(block, r"user \(the longest, 8600 us at 54\.000 ms, tid 1000\):\s+card_asio\.dll\+3000\s+card_asio\.dll\+4000")

    def test_other_processes_stacks_are_left_out(self):
        _, out = run(csw_stk, CUT, "--proc", "daw.exe")
        self.assertNotIn("other.exe", out)
        _, out = run(csw_stk, CUT, "--proc", "other.exe")
        self.assertIn("stacks of those threads after a WrResource wait: 0", out)

    def test_needs_a_process(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            csw_stk.main([CUT])


class Samples(unittest.TestCase):
    def test_counts_samples_per_module_and_lists_both_waits(self):
        code, out = run(csw_smp, CUT, "--proc", "daw.exe", "--proc", "other.exe")
        self.assertEqual(code, 0)
        self.assertRegex(out, r"daw\.exe audio threads \(2\): samples user 1, kernel 0")
        self.assertIn("user modules: card_asio.dll x1", out)
        self.assertIn("WrResource waits of the processes' threads: 2", out)
        self.assertRegex(out, r"x\s+1 total\s+8600 us\s+daw\.exe:1000\s+made ready by other\.exe:2000\(gui\)")


class Dump(unittest.TestCase):
    def test_lists_rows_in_the_window_only(self):
        code, out = run(csw_dump, CUT, "44", "55", "--proc", "daw.exe", "--cpus", "8-9")
        self.assertEqual(code, 0)
        self.assertIn("45.200 cpu 8 SAMPLE daw.exe:1000 at card_asio.dll+2000", out)
        self.assertIn("50.000 cpu 9 DPC    card.sys+20", out)
        self.assertIn("53.900 cpu 9 READY  daw.exe:1000 by other.exe:2000(gui)", out)
        self.assertNotIn("43.900", out)
        self.assertNotIn("55.500", out)

    def test_names_what_ran_before_the_window(self):
        _, out = run(csw_dump, CUT, "44", "55", "--proc", "daw.exe", "--cpus", "8-9")
        self.assertIn("cpu9: other.exe:2000(gui)", out.splitlines()[0])


class Lib(unittest.TestCase):
    def test_tag_parsing(self):
        self.assertEqual(cutlib.parse_tags(["A=Foo.exe"]), {"foo.exe": "A"})
        self.assertEqual(cutlib.parse_tags([]), {})
        with self.assertRaises(SystemExit):
            cutlib.parse_tags(["nonsense"])

    def test_module_lookup_splits_kernel_and_user_addresses(self):
        cut = cutlib.load(CUT)
        self.assertEqual(cut.mod(100, 0x10002000), "card_asio.dll+2000")
        self.assertEqual(cut.mod(0, 0xFFFFF80000200010), "card.sys+10")
        self.assertTrue(re.fullmatch(r"[0-9a-f]+", cut.mod(100, 0x5000)))


if __name__ == "__main__":
    unittest.main()
