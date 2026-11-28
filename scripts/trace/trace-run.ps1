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
# After a hit (Ring: the log says `trace saved`; Trigger: the out file has a `hit` line) it waits -GapMinutes, so one
# burst does not save twice. A pause after every child (-PauseSeconds) keeps a failing start (wpr: another trace runs)
# from becoming a tight loop. The scripts are run as they are, beside this file.
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
  if ($Role -eq 'Ring') { $script = 'event-trace.ps1'; $out = Join-Path $Tmp 'event-trace.txt'; $hitRx = '^\S+ trace saved '; $sargs = @() }
  else { $script = 'tone-trigger.ps1'; $out = Join-Path $Tmp 'tone-trigger.txt'; $hitRx = '^hit '; $sargs = @('-To', $To) }
  L "start $script"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Dir $script) @sargs -Minutes 1440 | Out-Null
  $rc = $LASTEXITCODE
  $hit = (Test-Path -LiteralPath $out) -and [bool](Select-String -LiteralPath $out -Pattern $hitRx -Quiet)
  L "$script ended exit $rc hit=$hit"
  if ($hit) { L "gap $GapMinutes min before arming again"; Start-Sleep -Seconds ($GapMinutes * 60) }
  Start-Sleep -Seconds $PauseSeconds
}
