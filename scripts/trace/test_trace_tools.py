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
import random
import re
import sys
import tempfile
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


READERS = [csw_an, csw_res, csw_smp, csw_stk, csw_dump]


def args_for(tool, path, image):
    """The command line of a reader for one image, whatever its positional arguments are."""
    if tool is csw_dump:
        return [path, "40", "50", "--proc", image]
    return [path, "--proc", image]


class Temp(unittest.TestCase):
    def cut_file(self, text):
        """Write a cut to a temp file (removed when the test ends)."""
        f = tempfile.NamedTemporaryFile("w", suffix=".cut", delete=False, encoding="utf-8", newline="\n")
        f.write(text)
        f.close()
        self.addCleanup(pathlib.Path(f.name).unlink)
        return f.name


def fixture_text():
    return pathlib.Path(CUT).read_text(encoding="utf-8")


def run(tool, *args):
    """Run a tool's main() on the fixture; return (exit code, stdout)."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = tool.main([*args])
    return code, out.getvalue()


class Analyse(Temp):
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


class Resources(Temp):
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


class Stacks(Temp):
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


class Samples(Temp):
    def test_counts_samples_per_module_and_lists_both_waits(self):
        code, out = run(csw_smp, CUT, "--proc", "daw.exe", "--proc", "other.exe")
        self.assertEqual(code, 0)
        self.assertRegex(out, r"daw\.exe audio threads \(2\): samples user 1, kernel 2")
        self.assertIn("user modules: card_asio.dll x1", out)
        self.assertIn("WrResource waits of the processes' threads: 3", out)
        self.assertRegex(out, r"x\s+1 total\s+8600 us\s+daw\.exe:1000\s+made ready by other\.exe:2000\(gui\)")


class Dump(Temp):
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


class Lib(Temp):
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


def tid_rows(tid, pid, name):
    return f"T,1,{pid},{tid},10001000,15,{name}"


class Findings(Temp):
    """Edge cases found in review: unknown process, damaged cuts, several pids of one image, option handling."""

    def test_unknown_process_ends_with_exit_1_in_every_reader(self):
        for tool in READERS:
            with self.subTest(tool=tool.__name__):
                code, out = run(tool, *args_for(tool, CUT, "nothere.exe"))
                self.assertEqual(code, 1)
                self.assertIn("no process nothere.exe in the cut", out)

    def test_one_missing_image_of_several_ends_with_exit_1(self):
        for tool in (csw_smp, csw_stk):
            code, out = run(tool, CUT, "--proc", "daw.exe", "--proc", "nothere.exe")
            self.assertEqual(code, 1)
            self.assertIn("no process nothere.exe in the cut", out)

    def test_empty_cut_ends_with_exit_1_not_a_traceback(self):
        path = self.cut_file("")
        for tool in READERS:
            with self.subTest(tool=tool.__name__):
                code, out = run(tool, *args_for(tool, path, "daw.exe"))
                self.assertEqual(code, 1)
                self.assertIn("no process daw.exe in the cut", out)

    def test_truncated_last_row_is_skipped_and_counted(self):
        path = self.cut_file(fixture_text() + "C,123,4")
        for tool in READERS:
            with self.subTest(tool=tool.__name__):
                code, out = run(tool, *args_for(tool, path, "daw.exe"))
                self.assertEqual(code, 0)
                self.assertIn("1 malformed rows skipped", out)

    def test_clean_cut_prints_no_malformed_line(self):
        for tool in READERS:
            _, out = run(tool, *args_for(tool, CUT, "daw.exe"))
            self.assertNotIn("malformed", out)

    def test_several_garbage_rows_are_all_counted_and_the_good_rows_still_read(self):
        # each reader counts only the row kinds it reads: csw_res not K, csw_stk not S
        path = self.cut_file(fixture_text() + "C,x,y\nS,1\nK,5\nM,1,2\nT,1\n")
        code, out = run(csw_res, path, "--proc", "daw.exe")
        self.assertEqual(code, 0)
        self.assertIn("4 malformed rows skipped", out)
        self.assertIn("the long ones (2)", out)
        code, out = run(csw_stk, path, "--proc", "daw.exe")
        self.assertEqual(code, 0)
        self.assertIn("4 malformed rows skipped", out)
        self.assertIn("stacks of those threads after a WrResource wait: 2", out)

    def test_every_pid_of_an_image_is_read(self):
        # a dead earlier daw.exe (pid 101) with an audio worker that waited 2 ms on a kernel resource
        extra = ("P,1,101,4,daw.exe\n" + tid_rows(1010, 101, "AudioCalc") + "\n"
                 + "M,10,101,10000000,100000,\\Device\\X\\card_asio.dll\n"
                 + "C,600000,12,1010,0,15,0,0,2,0\nC,610000,12,0,1010,0,15,27,5,0\nC,630000,12,1010,0,15,0,0,2,0\n"
                 + "S,615000,12,1010,10002000\n")
        path = self.cut_file(fixture_text() + extra)
        _, out = run(csw_res, path, "--proc", "daw.exe")
        self.assertIn("daw.exe pid 100,101: 3 audio threads", out)
        self.assertRegex(out, r"2000 us at\s+61\.000 ms\s+daw\.exe:1010\(AudioCalc\)\s+WrResource")
        _, out = run(csw_smp, path, "--proc", "daw.exe")
        self.assertRegex(out, r"daw\.exe audio threads \(3\): samples user 2")

    def test_audio_thread_guess_follows_period_ms(self):
        rows = ["P,1,10,4,p.exe", tid_rows(11, 10, ""), tid_rows(12, 10, "")]
        for k in range(60):
            for tid, step in ((11, 1.0), (12, 2.0)):
                ms = k * step
                rows.append(f"C,{int(ms * 10000)},3,{tid},0,15,0,0,2,0")
                rows.append(f"C,{int(ms * 10000) + 1000},3,0,{tid},0,15,6,5,0")
        path = self.cut_file("\n".join(rows) + "\n")
        _, out = run(csw_an, path, "--proc", "p.exe")
        self.assertIn("audio thread taken: p.exe:11", out)      # 1.0 ms is nearer the default 1.3333 than 2.0
        _, out = run(csw_an, path, "--proc", "p.exe", "--period-ms", "2.0")
        self.assertIn("audio thread taken: p.exe:12", out)

    def test_timeline_trim_matches_whole_tids_not_substrings(self):
        rows = ["P,1,10,4,p.exe", tid_rows(123, 10, "audio"), tid_rows(1234, 10, "other")]
        for k in range(10):
            ms = k * 1.5 if k < 5 else 20.0 + k * 1.5       # a gap of about 14 ms after 6.0 ms
            rows.append(f"C,{int(ms * 10000)},3,123,0,15,0,0,2,0")
            rows.append(f"C,{int(ms * 10000) + 1000},3,0,123,0,15,6,5,0")
        for k in range(8):
            ms = 8.0 + k
            rows.append(f"C,{int(ms * 10000)},4,1234,0,15,0,0,2,0")
            rows.append(f"C,{int(ms * 10000) + 1000},4,0,1234,0,15,6,5,0")
        path = self.cut_file("\n".join(rows) + "\n")
        _, out = run(csw_an, path, "--proc", "p.exe", "--tid", "123", "--timeline", "30")
        timeline = out[out.index("=== timeline"):out.index("=== inside the gap")]
        self.assertIn("showing the audio thread's and the card's only", timeline)
        self.assertIn("p.exe:123(audio)", timeline)
        self.assertNotIn("p.exe:1234", timeline)

    def test_from_and_to_limit_the_gaps(self):
        _, out = run(csw_an, CUT, "--proc", "daw.exe", "--tid", "1000", "--from", "50", "--to", "99")
        self.assertIn("gaps >= 3.0 ms: 0", out)
        _, out = run(csw_an, CUT, "--proc", "daw.exe", "--tid", "1000", "--from", "40", "--to", "46")
        self.assertIn("gaps >= 3.0 ms: 1", out)

    def test_tids_option_describes_threads_and_names_a_missing_one(self):
        _, out = run(csw_res, CUT, "--proc", "daw.exe", "--tids", "1000,9999")
        self.assertIn("thread 1000: daw.exe:1000 base pri 15 start card_asio.dll+1000; samples: [('card_asio.dll', 1)", out)
        self.assertIn("thread 9999: not in the thread list", out)

    def test_wait_without_a_switch_in_is_still_out_not_minus_one(self):
        _, out = run(csw_res, CUT, "--proc", "daw.exe")
        self.assertIn("WrResource           still out at the cut's end x1", out)
        self.assertIn("still out at the cut's end (1):", out)
        self.assertRegex(out, r"out at\s+98\.000 ms\s+daw\.exe:1001\(AudioCalc\)")
        self.assertNotIn("-1 us", out)
        self.assertIn("the long ones (2)", out)

    def test_kernel_samples_split_ntoskrnl_from_other_kernel_modules(self):
        _, out = run(csw_smp, CUT, "--proc", "daw.exe")
        self.assertIn("kernel modules: ntoskrnl.exe x1, card.sys x1", out)
        self.assertIn("kernel non-ntoskrnl addresses: card.sys+500 x1", out)


class DumpOptions(Temp):
    def test_all_adds_wakeups_of_other_processes_on_the_cpus(self):
        _, plain = run(csw_dump, CUT, "40", "56", "--proc", "daw.exe", "--cpus", "8-9")
        _, every = run(csw_dump, CUT, "40", "56", "--proc", "daw.exe", "--cpus", "8-9", "--all")
        self.assertNotIn("READY  other.exe:2000", plain)
        self.assertIn("41.000 cpu 9 READY  other.exe:2000(gui) by other.exe:2000(gui)", every)

    def test_no_samples_drops_the_sample_rows(self):
        _, with_s = run(csw_dump, CUT, "44", "56", "--proc", "daw.exe", "--cpus", "8-9")
        _, without = run(csw_dump, CUT, "44", "56", "--proc", "daw.exe", "--cpus", "8-9", "--no-samples")
        self.assertIn("SAMPLE", with_s)
        self.assertNotIn("SAMPLE", without)
        self.assertIn("SWITCH", without)

    def test_a_single_cpu_lists_only_that_cpu(self):
        _, out = run(csw_dump, CUT, "40", "56", "--proc", "daw.exe", "--cpus", "9")
        self.assertIn("running at the start: cpu9: ", out)
        self.assertNotIn("cpu8", out)
        self.assertIn("50.000 cpu 9 DPC", out)
        self.assertNotIn(" cpu 8 SAMPLE", out)

    def test_a_range_lists_each_cpu_of_it(self):
        _, out = run(csw_dump, CUT, "40", "56", "--proc", "daw.exe", "--cpus", "8-9")
        self.assertIn("cpu8: ", out.splitlines()[0])
        self.assertIn("cpu9: ", out.splitlines()[0])

    def test_bad_cpus_is_a_usage_error(self):
        for bad in ("a-b", "9-8"):
            with self.subTest(cpus=bad), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                csw_dump.main([CUT, "40", "56", "--proc", "daw.exe", "--cpus", bad])


class TruncatedAndShuffled(Temp):
    def test_last_row_without_its_newline_is_skipped_even_when_it_parses(self):
        # an S row with all its fields, but cut inside the address: it parses, with a wrong address
        base = fixture_text()
        path = self.cut_file(base + "S,455000,8,1000,FFFFF800")
        cut = cutlib.load(path)
        self.assertEqual(cut.skipped, 1)
        self.assertNotIn(0xFFFFF800, [r[3] for r in cut.sm])
        self.assertEqual(len(cut.sm), len(cutlib.load(CUT).sm))
        _, out = run(csw_res, path, "--proc", "daw.exe")
        self.assertIn("1 malformed rows skipped", out)

    def test_a_cut_with_its_final_newline_counts_nothing(self):
        self.assertEqual(cutlib.load(CUT).skipped, 0)

    def test_malformed_row_outside_the_window_is_counted(self):
        path = self.cut_file(fixture_text() + "C,990000,x,y\n")
        _, out = run(csw_dump, path, "44", "55", "--proc", "daw.exe", "--cpus", "8-9")
        self.assertIn("1 malformed rows skipped", out)
        self.assertEqual(cutlib.load(path, window=(44, 55)).skipped, 1)

    def test_rows_in_any_order_read_like_rows_in_time_order(self):
        lines = fixture_text().splitlines()
        timed = [i for i, l in enumerate(lines) if l[0] in "CRSID"]
        rows = [lines[i] for i in timed]
        random.Random(207).shuffle(rows)
        for i, r in zip(timed, rows):
            lines[i] = r
        self.assertNotEqual(lines, fixture_text().splitlines())
        shuffled = self.cut_file("\n".join(lines) + "\n")
        for args in (["--tid", "1000"], []):
            _, ordered = run(csw_an, CUT, "--proc", "daw.exe", *args)
            _, mixed = run(csw_an, shuffled, "--proc", "daw.exe", *args)
            self.assertEqual(ordered, mixed)
        self.assertIn("0.0 .. 99.4 ms", mixed)


if __name__ == "__main__":
    unittest.main()
