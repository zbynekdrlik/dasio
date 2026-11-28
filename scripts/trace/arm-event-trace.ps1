# dasio#207 (28.11.2026): the event trace comes back by itself after a reboot, a hit or a timeout. Run it once, as
# administrator, on each PC; it copies the scripts to -Dir (a task must not depend on a repo checkout) and registers
# a SYSTEM scheduled task that runs trace-run.ps1 (boot + a repetition every -EveryMinutes that only brings back a crashed
# runner; MultipleInstances IgnoreNew, no time limit), then starts it unless -NoStart.
#   DAW PC (the ring):        arm-event-trace.ps1 -Role Ring
#   watching PC (the trigger): arm-event-trace.ps1 -Role Trigger -To <host or address of the DAW PC>
#   remove:                    arm-event-trace.ps1 -Role Ring|Trigger -Remove      (ends the runner, the saved traces stay)
# A planned probe: create C:\ProgramData\dasio\tmp\event-trace.stop (the ring cancels within a second and stays off),
# delete it after (armed again within 30 s). More in .claude/rules/kernel-trace.md. Limits (-KeepSaves, -MinFreeGB,
# -GapMinutes) are passed on to trace-run.ps1.
param([Parameter(Mandatory)][ValidateSet('Ring', 'Trigger')][string]$Role, [string]$To = '', [switch]$Remove, [switch]$NoStart,
      [int]$KeepSaves = 6, [int]$MinFreeGB = 20, [int]$GapMinutes = 10, [int]$EveryMinutes = 5,
      [string]$Dir = 'C:\ProgramData\dasio\trace')
$ErrorActionPreference = 'Stop'
# Where event-trace.ps1 and tone-trigger.ps1 keep their saves, logs and the stop flag by default.
$Tmp = 'C:\ProgramData\dasio\tmp'
if ($Role -eq 'Trigger' -and -not $Remove -and -not $To) { throw '-To (the DAW PC) is required for -Role Trigger' }
$name = if ($Role -eq 'Ring') { 'dasio-event-trace-ring' } else { 'dasio-tone-trigger' }
$here = $PSScriptRoot
if ($Remove) {
  if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $name -Confirm:$false
  }
  # The runner and its children are plain processes, outside the task's own stop. The runner first, so nothing re-arms;
  # then the ring is asked to cancel its wpr trace through the stop flag (a hard kill would leave wpr tracing).
  function Kill([string]$rx) {
    Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" | Where-Object { $_.CommandLine -match $rx -and $_.ProcessId -ne $PID } |
      ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
  }
  Kill 'trace-run\.ps1'
  if ($Role -eq 'Ring') {
    $flag = Join-Path $Tmp 'event-trace.stop'; New-Item -ItemType File -Force -Path $flag | Out-Null
    Start-Sleep -Seconds 3; Remove-Item -LiteralPath $flag
    Kill 'event-trace\.ps1'
  } else { Kill 'tone-trigger\.ps1' }
  "removed $name"; return
}
New-Item -ItemType Directory -Force -Path $Dir, $Tmp | Out-Null
$files = 'trace-run.ps1', 'event-trace.ps1', 'tone-trigger.ps1'
foreach ($f in $files) { Copy-Item -LiteralPath (Join-Path $here $f) -Destination (Join-Path $Dir $f) -Force }
# event-trace.ps1's default -Profile is the tmp folder's copy.
Copy-Item -LiteralPath (Join-Path $here 'dasio-csw.wprp') -Destination (Join-Path $Tmp 'dasio-csw.wprp') -Force
$arg = "-NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Dir 'trace-run.ps1')`" -Role $Role -KeepSaves $KeepSaves -MinFreeGB $MinFreeGB -GapMinutes $GapMinutes"
if ($Role -eq 'Trigger') { $arg += " -To `"$To`"" }
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg
$triggers = @((New-ScheduledTaskTrigger -AtStartup),
              (New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes $EveryMinutes)))
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $action -Trigger $triggers -Settings $settings -Principal $principal -Force | Out-Null
"registered $name: powershell.exe $arg"
if (-not $NoStart) { Start-ScheduledTask -TaskName $name; "started $name (log: $Tmp\trace-run-$($Role.ToLower()).txt)" }
