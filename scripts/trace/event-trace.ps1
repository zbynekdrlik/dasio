# dasio#207 (27.11.2026): a context-switch trace kept in memory during a service, saved once at the first missed
# buffer the hall can hear. 26.11.2026's sermon had two master misses (HAND 1 sync, 17:03:23Z and 17:18:37Z) and no
# trace. Run it on the DAW PC; on the PC that watches its outputs run
#   tone-trigger.ps1 -To <this PC> -Minutes <same>      (its default -Match is the service's: breaks, loops, a loud step)
# At the trigger's UDP packet on -Port the ring is saved (wpr -stop, the last ~14 s) and the script ends: one trace per
# run, so the save's disk write happens once. No packet in -Minutes: the trace is cancelled, no file. A stop flag
# (-Flag) cancels too. Without -Etl the save is C:\ProgramData\dasio\tmp\event-<yyyyMMdd-HHmmss>.etl (UTC, the moment of
# the trigger), so a second armed run never overwrites the first's trace (27.11.2026: the 20:21Z TX-late trace was lost to
# the 21:27Z save). An existing file, also an explicit -Etl, is never overwritten: -2, -3 ... is appended. The log's
# `trace saved <path>` line names the file.
#   event-trace.ps1 [-Minutes 240] [-Etl ...] [-Profile ...] [-Log ...] [-Flag ...] [-Port 39099]
param([int]$Minutes = 240, [string]$Etl = '',
      [string]$Profile = 'C:\ProgramData\dasio\tmp\dasio-csw.wprp!DasioCsw', [string]$Log = 'C:\ProgramData\dasio\tmp\event-trace.txt',
      [string]$Flag = 'C:\ProgramData\dasio\tmp\event-trace.stop', [int]$Port = 39099)
$ErrorActionPreference = 'Stop'
(Get-Process -Id $PID).PriorityClass = 'Normal'
function L([string]$m) { "$((Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff')) $m" | Add-Content $Log }
# A path for the save that does not exist yet: $p itself, else $p with -2, -3 ... before the extension.
function Free-Path([string]$p) {
  if (-not (Test-Path $p)) { return $p }
  $d = [IO.Path]::GetDirectoryName($p); $n = [IO.Path]::GetFileNameWithoutExtension($p); $x = [IO.Path]::GetExtension($p)
  for ($i = 2; ; $i++) { $c = Join-Path $d "$n-$i$x"; if (-not (Test-Path $c)) { return $c } }
}
function Wpr([string[]]$WprArgs) {
  # wpr writes progress to stderr: under Stop that would throw.
  $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { & wpr.exe @WprArgs 2>&1 | Out-Null; return $LASTEXITCODE } finally { $ErrorActionPreference = $old }
}
"start $(Get-Date -Format o)" | Set-Content $Log
if (Test-Path $Flag) { Remove-Item $Flag }
$u = $null; $traced = $false
try {
  $u = New-Object Net.Sockets.UdpClient($Port)
  $rc = Wpr @('-start', $Profile)
  if ($rc -ne 0) { throw "wpr -start exit $rc" }
  $traced = $true; L 'trace on'
  $end = [DateTime]::UtcNow.AddMinutes($Minutes); $hit = $null
  while ([DateTime]::UtcNow -lt $end -and -not $hit) {
    if (Test-Path $Flag) { L 'flag seen'; break }
    if ($u.Available -gt 0) {
      # Only tone-trigger.ps1's packet ('event <log time>') saves the trace; any other packet is logged and skipped.
      $ep = $null; $got = [Text.Encoding]::ASCII.GetString($u.Receive([ref]$ep))
      if ($got.StartsWith('event ')) { $hit = "$got from $ep"; break } else { L "skipped a packet from $ep" }
    }
    Start-Sleep -Milliseconds 25
  }
  if ($hit) {
    L "trigger: $hit"
    $save = if ($Etl) { $Etl } else { 'C:\ProgramData\dasio\tmp\event-' + (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss') + '.etl' }
    $save = Free-Path $save
    $rc = Wpr @('-stop', $save, '-skipPdbGen')
    # A failed stop leaves the trace running: only an exit of 0 hands it off, else finally cancels it.
    if ($rc -eq 0) { $traced = $false; L "trace saved $save" } else { L "trace stop failed exit $rc" }
  } elseif (-not (Test-Path $Flag)) { L 'no trigger in the window' }
} catch { L ('FAILED: ' + $_.Exception.Message)
} finally {
  if ($traced) {
    try { $rc = Wpr @('-cancel'); L "trace cancelled exit $rc" } catch { L ('cancel failed: ' + $_.Exception.Message) }
  }
  if ($u) { $u.Close() }; L 'end'
}
