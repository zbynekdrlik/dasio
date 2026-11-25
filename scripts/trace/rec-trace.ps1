# dasio#207 (25.11.2026): the player's record starts run under a context-switch trace kept in memory, stopped by a
# tone watch on another PC.
# The watcher (tone-trigger.ps1 on the PC that plays the tone) sends a UDP packet to this PC's -Port when the
# watched tone breaks or a program output loops; this script then stops the trace at once (wpr -stop), so the
# event is still in every CPU's ring (about 14 s with dasio-csw.wprp). Needs AbleSet's OSC on this PC
# (ableset-osc.ps1, same folder or -Osc).
#   rec-trace.ps1 -GoLive 2 -Songs 7,3,6,4,5 -RecStop 8
# GO LIVE (song -GoLive), then per cycle: jump (the -Songs in turn), 0.5 s, play 1.5 s with record armed, stop, 1 s.
# REC STOP at the end (song -RecStop). No event in -Max cycles: the trace is cancelled, no file.
# -GoLive, -Songs and -RecStop are AbleSet setlist indexes of the set being tested; there is no default.
param([Parameter(Mandatory)][int]$GoLive, [Parameter(Mandatory)][int[]]$Songs, [Parameter(Mandatory)][int]$RecStop,
      [int]$Max = 400, [string]$Etl = 'C:\ProgramData\dasio\tmp\csw.etl', [string]$Profile = 'C:\ProgramData\dasio\tmp\dasio-csw.wprp!DasioCsw',
      [string]$Log = 'C:\dasio-runs\seq-rec-trace.txt', [string]$Flag = 'C:\dasio-runs\stop.flag', [string]$Osc = 'C:\dasio-runs\ableset-osc.ps1',
      [int]$ReplyPort = 39083, [int]$Port = 39099)
$ErrorActionPreference = 'Stop'
function L([string]$m) { "$((Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff')) $m" | Add-Content $Log }
function Send([string]$a, [int]$i = [int]::MinValue) { if ($i -eq [int]::MinValue) { & $Osc -Address $a -ReplyPort $ReplyPort | Out-Null } else { & $Osc -Address $a -Int $i -ReplyPort $ReplyPort | Out-Null } }
"start $(Get-Date -Format o)" | Set-Content $Log
if (Test-Path $Flag) { Remove-Item $Flag }
$u = New-Object Net.Sockets.UdpClient($Port)
$script:hit = ''
# Sleeps $ms in 25 ms steps; $true as soon as the trigger packet is here.
function Nap([int]$ms) {
  $end = [DateTime]::UtcNow.AddMilliseconds($ms)
  while ([DateTime]::UtcNow -lt $end) {
    if ($u.Available -gt 0) { $ep = $null; $script:hit = [Text.Encoding]::ASCII.GetString($u.Receive([ref]$ep)); return $true }
    Start-Sleep -Milliseconds 25
  }
  return $false
}
$traced = $false; $k = 0
try {
  & wpr.exe -start $Profile 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "wpr -start exit $LASTEXITCODE" }
  $traced = $true; L 'trace on'
  Start-Sleep 15
  while ($u.Available -gt 0) { $ep = $null; [void]$u.Receive([ref]$ep) }
  Send /setlist/jumpToSong $GoLive; L "jump $GoLive"; Start-Sleep 1
  Send /global/play; L 'play GO LIVE'; Start-Sleep -Milliseconds 1600
  Send /global/stop; L 'stop'; Start-Sleep 3
  $got = $false
  for ($k = 0; $k -lt $Max; $k++) {
    if (Test-Path $Flag) { L 'flag seen'; break }
    $s = $Songs[$k % $Songs.Count]
    Send /setlist/jumpToSong $s; L "jump $s"; if (Nap 500) { $got = $true; break }
    Send /global/play; L 'play s'; if (Nap 1500) { $got = $true; break }
    Send /global/stop; L 'stop'; if (Nap 1000) { $got = $true; break }
  }
  if ($got) {
    L "trigger: $($script:hit)"
    & wpr.exe -stop $Etl -skipPdbGen 2>&1 | Out-Null
    L "trace saved exit $LASTEXITCODE"; $traced = $false
  }
  L "cycles $k"
  Send /global/stop; Start-Sleep 1
  L 'REC STOP'
  Send /setlist/jumpToSong $RecStop; L "jump $RecStop"; Start-Sleep 1
  Send /global/play; L 'play REC STOP'; Start-Sleep -Milliseconds 1500
} catch { L ('FAILED: ' + $_.Exception.Message)
} finally {
  Send /global/stop
  if ($traced) { & wpr.exe -cancel 2>&1 | Out-Null; L 'trace cancelled' }
  $u.Close(); L 'end'
}
