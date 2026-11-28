# dasio#207 (28.11.2026): the event trace comes back by itself after a reboot, a hit or a timeout. Run it once, as
# administrator, on each PC; it copies the scripts to -Dir (a task must not depend on a repo checkout) and registers
# a SYSTEM scheduled task that runs trace-run.ps1 (boot + a repetition every -EveryMinutes that only brings back a crashed
# runner; MultipleInstances IgnoreNew, no time limit), then starts it unless -NoStart.
#   DAW PC (the ring):        arm-event-trace.ps1 -Role Ring
#   watching PC (the trigger): arm-event-trace.ps1 -Role Trigger -To <host or address of the DAW PC>
#   remove:                    arm-event-trace.ps1 -Role Ring|Trigger -Remove      (ends this role's runner, the saved traces stay)
# A planned probe: create C:\ProgramData\dasio\tmp\event-trace.stop (the ring cancels within a second and stays off),
# delete it after (armed again within 30 s). More in .claude/rules/kernel-trace.md. Limits (-KeepSaves, -MinFreeGB,
# -GapMinutes) are passed on to trace-run.ps1. The ring role also opens UDP 39099 inbound in the Windows firewall (a rule
# for that port only; -Remove takes it away); the trigger's packets reach a SYSTEM task's powershell only through it.
# -Remove of a ring: the stop flag is made first (kept if the operator had made it; one made here is always taken back), the
# ring is awaited (30 s), the task is stopped and removed, this role's runner and its children are ended, `wpr -cancel` clears
# what a hard end leaves.
param([Parameter(Mandatory)][ValidateSet('Ring', 'Trigger')][string]$Role, [string]$To = '', [switch]$Remove, [switch]$NoStart,
      [int]$KeepSaves = 6, [int]$MinFreeGB = 20, [int]$GapMinutes = 10, [int]$EveryMinutes = 5,
      [string]$Dir = 'C:\ProgramData\dasio\trace')
$ErrorActionPreference = 'Stop'
# Where event-trace.ps1 and tone-trigger.ps1 keep their saves, logs and the stop flag by default.
$Tmp = 'C:\ProgramData\dasio\tmp'
if ($Role -eq 'Trigger' -and -not $Remove -and -not $To) { throw '-To (the DAW PC) is required for -Role Trigger' }
$name = if ($Role -eq 'Ring') { 'dasio-event-trace-ring' } else { 'dasio-tone-trigger' }
$here = $PSScriptRoot
$Port = 39099  # event-trace.ps1's default -Port
$ruleName = "dasio event trace (UDP $Port)"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
function Say([string]$m) { $m; "$((Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss')) $m" | Add-Content -LiteralPath (Join-Path $Tmp 'arm-event-trace.txt') }
# powershell.exe processes whose command line matches $Rx, never this installer (its own name matches the scripts').
function Get-TraceProc([string]$Rx) {
  @(Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" |
    Where-Object { $_.CommandLine -match $Rx -and $_.CommandLine -notmatch 'arm-event-trace\.ps1' -and $_.ProcessId -ne $PID })
}
function Stop-TraceProc($Procs) { foreach ($p in $Procs) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue } }
if ($Remove) {
  # This role's runner (its task's command line carries `-Role <role>`) and the children of those runners only.
  $runners = Get-TraceProc "trace-run\.ps1.* -Role $Role\b"
  $childRx = if ($Role -eq 'Ring') { 'event-trace\.ps1' } else { 'tone-trigger\.ps1' }
  $kids = @(Get-TraceProc $childRx | Where-Object { $runners.ProcessId -contains $_.ParentProcessId })
  $flag = Join-Path $Tmp 'event-trace.stop'
  $madeFlag = $false; $ringEnded = $true
  try {
    if ($Role -eq 'Ring') {
      # Pause the ring through the stop flag first (it cancels its wpr trace itself, the runner does not re-arm), a flag the
      # operator already made stays. A flag made here is taken back in the finally, whatever fails.
      $madeFlag = -not (Test-Path -LiteralPath $flag)
      if ($madeFlag) { New-Item -ItemType File -Force -Path $flag | Out-Null }
      function Wait-RingEnd([int[]]$Pids, [int]$Seconds = 30) {
        $until = (Get-Date).AddSeconds($Seconds)
        while ((Get-Date) -lt $until) {
          if (-not $Pids -or -not (Get-Process -Id $Pids -ErrorAction SilentlyContinue)) { return $true }
          Start-Sleep -Milliseconds 500
        }
        return $false
      }
      $kidIds = @($kids | ForEach-Object { $_.ProcessId })
      $ringEnded = Wait-RingEnd $kidIds
    }
    if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) {
      Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
      Unregister-ScheduledTask -TaskName $name -Confirm:$false
    }
    Stop-TraceProc $runners; Stop-TraceProc $kids
    if (-not $ringEnded) { Say 'the ring did not end in 30 s: ended by force' }
    if ($Role -eq 'Ring') {
      # A ring ended by force leaves its wpr session: clear it (exit code ignored, non-zero = none was running).
      $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
      try { & wpr.exe -cancel 2>&1 | Out-Null; Say "wpr -cancel exit $LASTEXITCODE" } finally { $ErrorActionPreference = $old }
      if (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue) { Remove-NetFirewallRule -DisplayName $ruleName; Say "firewall rule removed: $ruleName" }
    }
    Say "removed ${name}"
  } finally {
    if ($madeFlag) { Remove-Item -LiteralPath $flag -ErrorAction SilentlyContinue }
  }
  return
}
$files = 'trace-run.ps1', 'trace-run-lib.ps1', 'event-trace.ps1', 'tone-trigger.ps1'
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
foreach ($f in $files) { Copy-Item -LiteralPath (Join-Path $here $f) -Destination (Join-Path $Dir $f) -Force }
# event-trace.ps1's default -Profile is the tmp folder's copy.
Copy-Item -LiteralPath (Join-Path $here 'dasio-csw.wprp') -Destination (Join-Path $Tmp 'dasio-csw.wprp') -Force
if ($Role -eq 'Ring') {
  if (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue) { Say "firewall rule exists: $ruleName" }
  else {
    New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow -Protocol UDP -LocalPort $Port -Profile Any | Out-Null
    Say "firewall rule added: $ruleName (inbound UDP $Port)"
  }
}
$arg = "-NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Dir 'trace-run.ps1')`" -Role $Role -KeepSaves $KeepSaves -MinFreeGB $MinFreeGB -GapMinutes $GapMinutes"
if ($Role -eq 'Trigger') { $arg += " -To `"$To`"" }
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg
$triggers = @((New-ScheduledTaskTrigger -AtStartup),
              (New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes $EveryMinutes)))
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $action -Trigger $triggers -Settings $settings -Principal $principal -Force | Out-Null
Say "registered ${name}: powershell.exe $arg"
if (-not $NoStart) { Start-ScheduledTask -TaskName $name; Say "started ${name} (log: $Tmp\trace-run-$($Role.ToLower()).txt)" }
