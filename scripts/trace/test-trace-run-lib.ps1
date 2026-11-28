# dasio#207: the decisions of trace-run.ps1 (trace-run-lib.ps1) against fixture logs, in Windows PowerShell 5.1 as on the PCs.
# CI runs it (job ps-parse); no ETW, no task scheduler. Exits 1 and names each failed check.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'trace-run-lib.ps1')
$script:failed = 0
function Check([string]$Name, [bool]$Ok) {
  if ($Ok) { "ok   $Name" } else { $script:failed++; "FAIL $Name" }
}
$root = Join-Path ([IO.Path]::GetTempPath()) ("trace-run-lib-test-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $root | Out-Null
function Fixture([string]$Name, [string[]]$Lines, [int]$AgeMinutes) {
  $p = Join-Path $root $Name
  Set-Content -LiteralPath $p -Value $Lines
  (Get-Item -LiteralPath $p).LastWriteTimeUtc = [DateTime]::UtcNow.AddMinutes(-$AgeMinutes)
  return $p
}
$since = [DateTime]::UtcNow.AddMinutes(-5)
$ringRx = '^\S+ trace saved '
try {
  # --- a hit is only read from what this child wrote -------------------------------------------------------------
  $fresh = Fixture 'ring-fresh.txt' @('start 2026-11-28T10:00:00', '10:00:01.000 trace on', '10:03:00.000 trace saved C:\x\event-1.etl', '10:03:01.000 end') 1
  Check 'a fresh trace saved line is a hit' (Test-Hit $fresh $ringRx $since)
  $stale = Fixture 'ring-stale-hit.txt' @('start 2026-11-28T09:00:00', '09:03:00.000 trace saved C:\x\event-0.etl', 'start 2026-11-28T10:00:00', '10:00:01.000 trace on', '10:30:00.000 no trigger in the window', '10:30:01.000 end') 1
  Check 'a hit before the last start line is no hit' (-not (Test-Hit $stale $ringRx $since))
  Check 'the stale run keeps only its own lines' ((Get-RunLines $stale $since).Count -eq 4)
  $old = Fixture 'ring-old.txt' @('start 2026-11-28T08:00:00', '08:03:00.000 trace saved C:\x\event-0.etl', '08:03:01.000 end') 10
  Check 'a log not written since the child began is no hit' (-not (Test-Hit $old $ringRx $since))
  Check 'a log not written since the child began has no lines' ((Get-RunLines $old $since).Count -eq 0)
  Check 'a missing log is no hit' (-not (Test-Hit (Join-Path $root 'nothing.txt') $ringRx $since))
  $empty = Fixture 'ring-empty.txt' @() 1
  Check 'an empty log has no lines' ((Get-RunLines $empty $since).Count -eq 0)
  Check 'an empty log is no hit' (-not (Test-Hit $empty $ringRx $since))
  $trig = Fixture 'trigger-hit.txt' @('start 10:00:00.000', 'target mbc = 10.0.0.1', 'hit 10:01:00.000 a watched test tone broke', 'end') 1
  Check 'a trigger hit line is a hit' (Test-Hit $trig '^hit ' $since)
  $trigTimeout = Fixture 'trigger-timeout.txt' @('start 10:00:00.000', 'target mbc = 10.0.0.1', 'timeout', 'end') 1
  Check 'a trigger timeout is no hit' (-not (Test-Hit $trigTimeout '^hit ' $since))

  # --- what refuses to arm ---------------------------------------------------------------------------------------
  $tmp = Join-Path $root 'tmp'; New-Item -ItemType Directory -Force -Path $tmp | Out-Null
  Check 'a trigger is never refused' ((Get-Refusal -Role Trigger -Tmp $tmp -KeepSaves 1 -MinFreeGB 1000 -FreeGB 0) -eq '')
  Check 'an empty tmp folder with room arms the ring' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 3 -MinFreeGB 20 -FreeGB 100) -eq '')
  1..2 | ForEach-Object { Set-Content -LiteralPath (Join-Path $tmp "event-2026112$_.etl") -Value 'x' }
  Check 'fewer saves than the limit still arm' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 3 -MinFreeGB 20 -FreeGB 100) -eq '')
  Set-Content -LiteralPath (Join-Path $tmp 'event-20261123.etl') -Value 'x'
  Check 'saves at the limit refuse' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 3 -MinFreeGB 20 -FreeGB 100) -match 'saved traces')
  Check 'the refusal deletes nothing' (@(Get-ChildItem -LiteralPath $tmp -Filter 'event-*.etl').Count -eq 3)
  Check 'three saves under a limit of four arm' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 4 -MinFreeGB 20 -FreeGB 100) -eq '')
  Check 'too little free space refuses' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 9 -MinFreeGB 20 -FreeGB 5) -match 'GB free')
  Check 'exactly the floor arms' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 9 -MinFreeGB 20 -FreeGB 20) -eq '')
  Set-Content -LiteralPath (Join-Path $tmp 'event-trace.stop') -Value ''
  Check 'the stop flag refuses first' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 1 -MinFreeGB 20 -FreeGB 1) -match 'stop flag')
  Check 'the flag is not deleted by the check' (Test-Path -LiteralPath (Join-Path $tmp 'event-trace.stop'))
  Remove-Item -LiteralPath (Join-Path $tmp 'event-trace.stop')
  Check 'the real free space of the drive is read when none is given' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 9 -MinFreeGB 0) -eq '')
  Check 'and a floor no drive meets refuses' ((Get-Refusal -Role Ring -Tmp $tmp -KeepSaves 9 -MinFreeGB 100000000) -match 'GB free')
} finally {
  Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
if ($script:failed -gt 0) { "$($script:failed) check(s) failed"; exit 1 }
'all checks passed'
