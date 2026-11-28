# dasio#207 (28.11.2026): keeps the event trace armed without a hand start. Registered as a SYSTEM task by
# arm-event-trace.ps1 (boot + a repetition that only brings back a crashed runner); it loops by itself, so after a hit
# or a timeout the ring is dark for seconds, not for the repetition interval.
#   -Role Ring     on the DAW PC: event-trace.ps1 -Minutes 1440 (the ring, saved at the trigger's packet)
#   -Role Trigger  on the watching PC: tone-trigger.ps1 -To <DAW PC> -Minutes 1440
# Before every start it checks, and arms nothing while one holds (the state is logged when it changes, re-checked every
# 30 s, nothing is deleted, ever):
#   - Ring: the stop flag (event-trace.stop) exists: a planned probe. Delete the flag to arm again.
#   - Ring: -KeepSaves or more event-*.etl in the tmp folder: read them off the PC and remove them by hand.
#   - Ring: less than -MinFreeGB free on the tmp folder's drive (a save writes the whole ring, 512 MB).
# After a hit (Ring: the log says `trace saved`; Trigger: the out file has a `hit` line, both read only from what THIS
# child wrote) it waits -GapMinutes, so one burst does not save twice. A child that ended before 60 s (a failed start: wpr
# says another trace runs, a name that does not resolve) is followed by a pause (-PauseSeconds), so it is no tight loop; one
# that ran to its timeout is started again at once. Before every ring start a stale wpr session is cancelled (a ring killed
# hard leaves one, and every later start would fail). The ring is started with -KeepFlag (a flag made after the check
# above is not wiped). The scripts are run as they are, beside this file.
#   trace-run.ps1 -Role Ring|Trigger [-To <host>] [-KeepSaves 6] [-MinFreeGB 20] [-GapMinutes 10] [-Dir ...]
param([Parameter(Mandatory)][ValidateSet('Ring', 'Trigger')][string]$Role, [string]$To = '',
      [int]$KeepSaves = 6, [int]$MinFreeGB = 20, [int]$GapMinutes = 10, [int]$PauseSeconds = 30,
      [string]$Dir = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
# The folder the two scripts write to by default (their saves, logs and the stop flag).
$Tmp = 'C:\ProgramData\dasio\tmp'
if ($Role -eq 'Trigger' -and -not $To) { throw '-To (the DAW PC) is required for -Role Trigger' }
$Run = Join-Path $Tmp "trace-run-$($Role.ToLower()).txt"
$Flag = Join-Path $Tmp 'event-trace.stop'
function L([string]$m) { "$((Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss')) $m" | Add-Content -LiteralPath $Run }
# Why nothing may be armed now, or '' when all is well.
function Refusal {
  if ($Role -ne 'Ring') { return '' }
  if (Test-Path -LiteralPath $Flag) { return 'the stop flag is there (a planned probe): delete event-trace.stop to arm again' }
  $n = @(Get-ChildItem -LiteralPath $Tmp -Filter 'event-*.etl' -File -ErrorAction SilentlyContinue).Count
  if ($n -ge $KeepSaves) { return "$n saved traces (limit $KeepSaves): read them off the PC and remove them, nothing is deleted here" }
  $free = [Math]::Floor((New-Object IO.DriveInfo((Split-Path -Qualifier (Resolve-Path -LiteralPath $Tmp)))).AvailableFreeSpace / 1GB)
  if ($free -lt $MinFreeGB) { return "$free GB free (limit $MinFreeGB GB)" }
  return ''
}
# The lines the child wrote in ITS run: none when the file was not written since $Since (a child that failed before it
# rewrote its log leaves the last run's lines there), else the lines from the last `start` line on.
function Get-RunLines([string]$Path, [datetime]$Since) {
  if (-not (Test-Path -LiteralPath $Path)) { return @() }
  if ((Get-Item -LiteralPath $Path).LastWriteTimeUtc -lt $Since.ToUniversalTime()) { return @() }
  $lines = @(Get-Content -LiteralPath $Path); $from = 0
  for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^start ') { $from = $i } }
  return @($lines[$from..($lines.Count - 1)])
}
function Test-Hit([string]$Path, [string]$Rx, [datetime]$Since) {
  foreach ($l in (Get-RunLines $Path $Since)) { if ($l -match $Rx) { return $true } }
  return $false
}
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
L "runner start role=$Role keep=$KeepSaves minfreegb=$MinFreeGB gap=$GapMinutes"
$was = $null
while ($true) {
  $why = Refusal
  if ($why) {
    if ($why -ne $was) { L "not armed: $why" }
    $was = $why; Start-Sleep -Seconds $PauseSeconds; continue
  }
  if ($was) { L 'armed again' }; $was = $null
  if ($Role -eq 'Ring') {
    $script = 'event-trace.ps1'; $out = Join-Path $Tmp 'event-trace.txt'; $hitRx = '^\S+ trace saved '; $sargs = @('-KeepFlag')
    # wpr writes progress to stderr: under Stop that would throw.
    $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    try { & wpr.exe -cancel 2>&1 | Out-Null; L "wpr -cancel exit $LASTEXITCODE (0 = a stale session was cancelled)" } finally { $ErrorActionPreference = $old }
  } else { $script = 'tone-trigger.ps1'; $out = Join-Path $Tmp 'tone-trigger.txt'; $hitRx = '^hit '; $sargs = @('-To', $To) }
  L "start $script"
  $t0 = Get-Date
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Dir $script) @sargs -Minutes 1440 | Out-Null
  $rc = $LASTEXITCODE; $ran = (Get-Date) - $t0
  $hit = Test-Hit $out $hitRx $t0
  L "$script ended exit $rc after $([int]$ran.TotalSeconds) s hit=$hit"
  # Why a start failed (the child's own words), so the runner's log is enough to read.
  foreach ($l in @(Get-RunLines $out $t0 | Where-Object { $_ -match 'FAILED|trace stop failed|stop flag present|flag seen' } | Select-Object -First 3)) { L "child: $l" }
  if ($hit) { L "gap $GapMinutes min before arming again"; Start-Sleep -Seconds ($GapMinutes * 60) }
  if (-not $hit -and $ran.TotalSeconds -ge 60) { L 'ran to its end: arming again at once' } else { Start-Sleep -Seconds $PauseSeconds }
}
