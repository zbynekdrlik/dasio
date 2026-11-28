"""Text guards on install.ps1 (dasio#28): an exe copy into Program Files survives a short hold, and a stopped service's process is waited for."""
import re
import unittest
from pathlib import Path

INSTALL = (Path(__file__).parent / "install.ps1").read_text(encoding="utf-8")
CODE = "\n".join(l for l in INSTALL.splitlines() if not l.lstrip().startswith("#"))


class InstallCopies(unittest.TestCase):
    def test_retry_function_is_bounded_logged_and_fails_loudly(self):
        m = re.search(r"function Copy-FileRetry\b.*?\n}\n", CODE, re.S)
        self.assertIsNotNone(m, "Copy-FileRetry is missing")
        body = m.group(0)
        self.assertRegex(body, r"-ge \d+")  # bounded tries
        self.assertIn("Write-Warning", body)  # each failure logged
        self.assertIn("throw", body)  # then fail loudly

    def test_exe_copies_use_the_retry(self):
        # No bare Copy-Item onto the shared bin, an instance exe, the keeper or a restored file.
        for line in CODE.splitlines():
            if "Copy-Item" in line:
                self.assertNotRegex(line, r"\$bin\b|\$instExe|\$keepExe|\$Path|\$beforeExe\b\s*$", f"a bare copy: {line.strip()}")
        for needle in (r"Copy-FileRetry .*\$bin\b", r"Copy-FileRetry .*\$instExe", r"Copy-FileRetry \$Saved \$Path", r"Copy-FileRetry .*\$keepExe"):
            self.assertRegex(CODE, needle)


class InstallStops(unittest.TestCase):
    def test_every_stop_service_waits_for_the_process_to_exit(self):
        stops = len(re.findall(r"Stop-Service\b", CODE))
        waits = len(re.findall(r"Wait-ServiceProcessExit\b", CODE)) - 1  # minus the definition
        self.assertGreaterEqual(stops, 2)
        self.assertEqual(stops, waits, "each Stop-Service needs its process-exit wait")

    def test_process_wait_is_bounded_and_names_the_pid(self):
        m = re.search(r"function Wait-ServiceProcessExit\b.*?\n}\n", CODE, re.S)
        self.assertIsNotNone(m, "Wait-ServiceProcessExit is missing")
        self.assertIn("ProcessId", INSTALL)
        self.assertRegex(m.group(0), r"Wait-Process .*-Timeout")
        self.assertIn("throw", m.group(0))


if __name__ == "__main__":
    unittest.main()
