# dasio#207 (25.11.2026): the stop trigger of a trace on another PC. Run it on the PC whose dasio plays the watched
# tone. It tails that dasio.log from its end; at the first `a watched test tone broke` or
# `a watched program output looped` line it sends a UDP packet to the PC that records (rec-trace.ps1 listens there
# on -Port) and ends. On CPUs 0-1 (2-3 are dasio's own).
#   tone-trigger.ps1 -To <host or address of the recording PC> [-Port 39099] [-Minutes 40] [-Out ...] [-Log ...]
param([Parameter(Mandatory)][string]$To, [int]$Port = 39099, [int]$Minutes = 40, [string]$Out = 'C:\ProgramData\dasio\tmp\tone-trigger.txt',
      [string]$Log = 'C:\ProgramData\dasio\instance-1\dasio.log')
$ErrorActionPreference = 'Stop'
(Get-Process -Id $PID).ProcessorAffinity = 3
"start $((Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff'))" | Set-Content $Out
$share = [IO.FileShare]'ReadWrite,Delete'
$u = New-Object Net.Sockets.UdpClient
$fs = $null; $buf = New-Object byte[] 65536; $carry = ''; $hit = $null; $turns = 0
# One decoder for the whole file: a read can end inside a multibyte UTF-8 character.
$dec = [Text.Encoding]::UTF8.GetDecoder()
$end = [DateTime]::UtcNow.AddMinutes($Minutes)
try {
  $fs = New-Object IO.FileStream($Log, [IO.FileMode]::Open, [IO.FileAccess]::Read, $share)
  $pos = $fs.Length
  while ([DateTime]::UtcNow -lt $end -and -not $hit) {
    $len = $fs.Length
    if ($len -gt $pos) {
      $fs.Position = $pos; $n = $fs.Read($buf, 0, [int][Math]::Min([int64]$buf.Length, $len - $pos)); $pos += $n
      $chars = New-Object char[] ($n + 4); $cn = $dec.GetChars($buf, 0, $n, $chars, 0)
      $lines = ($carry + [string]::new($chars, 0, $cn)).Split("`n"); $carry = $lines[$lines.Count - 1]
      for ($i = 0; $i -lt $lines.Count - 1; $i++) {
        if ($lines[$i] -match 'a watched test tone broke|a watched program output looped') { $hit = $lines[$i]; break }
      }
    } else {
      Start-Sleep -Milliseconds 40
      # The log rotates at 10 MB: the open handle then stays on the renamed file.
      if ((++$turns % 100) -eq 0 -and (New-Object IO.FileInfo($Log)).Length -lt $pos) {
        $fs.Close(); $fs = New-Object IO.FileStream($Log, [IO.FileMode]::Open, [IO.FileAccess]::Read, $share); $pos = 0; $carry = ''; $dec.Reset()
      }
    }
  }
  if ($hit) {
    $m = [Text.Encoding]::ASCII.GetBytes('event ' + $hit.Substring(0, [Math]::Min(27, $hit.Length)))
    1..3 | ForEach-Object { [void]$u.Send($m, $m.Length, $To, $Port); Start-Sleep -Milliseconds 20 }
    "hit $((Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff')) $hit" | Add-Content $Out
  } else { 'timeout' | Add-Content $Out }
} catch { "FAILED: $_" | Add-Content $Out
} finally { if ($fs) { $fs.Close() }; $u.Close(); 'end' | Add-Content $Out }
