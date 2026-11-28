"""Text guards on install.ps1 (dasio#28): an exe copy into Program Files survives a short hold, and a stopped service's process is waited for."""
import re
import unittest
from pathlib import Path

CI_TEST = (Path(__file__).parent / "ci-service-test.ps1").read_text(encoding="utf-8")

INSTALL = (Path(__file__).parent / "install.ps1").read_text(encoding="utf-8")
LINES = [l for l in INSTALL.replace("\r\n", "\n").replace("`\n", " ").splitlines() if not l.lstrip().startswith("#")]
CODE = "\n".join(LINES)

# Copies that stay bare: the rollback copies read the old binary and config into the rollback folder
# (nothing is held there), and the config copy reads the caller's own file.
BARE_COPY_TARGETS = (r'\(Join-Path \$rollback "instance\.exe"\)', r'\(Join-Path \$rollback "device\.toml"\)', r"\$cfg -Force")


def body(name):
    m = re.search(rf"function {name}\b.*?\n}}\n", CODE, re.S)
    assert m, f"{name} is missing"
    return m.group(0)


class InstallCopies(unittest.TestCase):
    def test_retry_is_bounded_logged_fails_loudly_and_retries_only_io(self):
        b = body("Copy-FileRetry")
        self.assertRegex(b, r"-ge \d+")
        self.assertIn("Write-Warning", b)
        self.assertIn("IOException", b)
        self.assertIn("UnauthorizedAccessException", b)
        self.assertRegex(b, r"if \(-not \$held\) \{ throw \}")  # anything else fails at once
        # Only a sharing or lock violation is a held file: 32 and 33 of the low HResult word.
        self.assertRegex(b, r"HResult -band 0xFFFF")
        self.assertRegex(b, r"-in 32, ?33|-eq 32 -or .* -eq 33")
        self.assertRegex(b, r'throw "copying ')

    def test_every_other_copy_item_is_a_listed_bare_one(self):
        inside = body("Copy-FileRetry")
        for line in LINES:
            if "Copy-Item" in line and line.strip() not in inside:
                self.assertTrue(
                    any(re.search(t, line) for t in BARE_COPY_TARGETS),
                    f"a copy outside Copy-FileRetry: {line.strip()}",
                )

    def test_the_known_targets_go_through_the_retry(self):
        for needle in (r"Copy-FileRetry .*\$bin\b", r"Copy-FileRetry .*\$instExe", r"Copy-FileRetry \$Saved \$Path", r"Copy-FileRetry .*\$keepExe", r"Copy-FileRetry \$from \$to"):
            self.assertRegex(CODE, needle)


class InstallStops(unittest.TestCase):
    def stops(self):
        return [i for i, l in enumerate(LINES) if "Stop-Service" in l]

    def test_each_stop_is_preceded_by_a_process_read_and_followed_by_its_wait(self):
        stops = self.stops()
        self.assertGreaterEqual(len(stops), 2)
        for i in stops:
            self.assertTrue(any("Get-ServiceProcessInfo" in l for l in LINES[max(0, i - 4) : i]), f"no process read before: {LINES[i].strip()}")
            self.assertTrue(any("Wait-ServiceProcessExit" in l for l in LINES[i + 1 : i + 4]), f"no process wait after: {LINES[i].strip()}")

    def test_each_stop_is_inside_a_try_that_brings_the_service_back(self):
        # The instance's stop: a catch that starts it again. The paused ones: the `finally` over `$stopped`.
        first = self.stops()[0]
        self.assertTrue(any(l.strip() == "try {" for l in LINES[first - 4 : first]), "the instance's stop is outside a try")
        after = "\n".join(LINES[first : first + 14])
        catch = re.search(r"\} catch \{(.*?)\n    \}\n", after, re.S)
        self.assertIsNotNone(catch, "the instance's stop has no catch")
        self.assertIn("Start-Service -Name $service", catch.group(1))
        second = self.stops()[1]
        self.assertTrue(any(l.strip() == "try {" for l in LINES[:second][-8:]), "the paused instances' stop is outside a try")
        self.assertRegex(CODE, r"\$stopped \+= \$s\s*\n\s*Stop-Service -Name \$s\.Name")
        fin = re.search(r"\} finally \{\s*\n\s*\$restartFailures = @\(\)\s*\n\s*foreach \(\$s in \$stopped\) \{(.*?)\n    \}\n(.*?)\n\}\n", CODE, re.S)
        self.assertIsNotNone(fin, "no finally over $stopped")
        loop, tail = fin.groups()
        # Each restart has its own try/catch; failures are collected and ONE message is thrown, original error kept.
        self.assertIn("try {", loop)
        self.assertIn("catch {", loop)
        self.assertIn("throw", tail)
        self.assertRegex(CODE, r"catch \{\s*\n\s*\$copyFailure = \$_\.Exception\.Message\s*\n\s*throw")
        self.assertIn("$copyFailure", tail)

    def test_process_wait_is_bounded_names_the_pid_and_checks_the_start_time(self):
        b = body("Wait-ServiceProcessExit")
        self.assertIn("AddSeconds(15)", b)
        self.assertIn("StartTime", b)
        self.assertIn("throw", b)
        info = body("Get-ServiceProcessInfo")
        self.assertIn("StartTime", info)
        self.assertIn("Write-Warning", info)  # an unreadable start time is logged: the wait is by id only


class CiHeldCase(unittest.TestCase):
    def test_the_install_job_wait_is_bounded_and_the_job_is_stopped(self):
        self.assertRegex(CI_TEST, r"Wait-Job \$installJob -Timeout \d+")
        self.assertIn("Stop-Job $installJob", CI_TEST)
        # stopped also when the early wait throws: the catch around it holds the stop
        early = CI_TEST[CI_TEST.index("$installJob = Start-Job") :]
        catch = re.search(r"\} catch \{(.*?)\} finally", early, re.S)
        self.assertIsNotNone(catch)
        self.assertIn("Stop-Job $installJob", catch.group(1))


if __name__ == "__main__":
    unittest.main()
