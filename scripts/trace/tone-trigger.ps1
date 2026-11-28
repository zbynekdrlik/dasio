# dasio#207 (25.11.2026): the stop trigger of a trace on another PC. Run it on the PC whose dasio plays the watched
# tone. It tails that dasio.log from its end; at the first `a watched test tone broke` or
# `a watched program output looped` line it sends a UDP packet to the PC that records (rec-trace.ps1 listens there
# on -Port) and ends. On CPUs 0-1 (2-3 are dasio's own). The default -Match is the service's (event-trace.ps1):
# also `a watched output broke` (a host's missed buffer on real program audio, 26.11.2026's sermon), `a watched
# program output looped`, and a loud step, `output jumped` with `peak_dbfs` louder than -46 dBFS (-45.x up to 0.0, 27.11.2026;
# tracing prints `-30.0`, the regex takes `-30` too). An RX stall's cuts (`a watched output was cut while RX packets
# were missing ...`, `a watched program audio step came while RX packets were missing ...`) are none of these. -Match narrows or widens it.
# -To is resolved (IPv4) at the start: a name that does not resolve ends the script at once with `FAILED: ...` in -Out, not at the
# first hit.
#   tone-trigger.ps1 -To <host or address of the recording PC> [-Port 39099] [-Minutes 40] [-Match <regex>] [-Out ...] [-Log ...]
param([Parameter(Mandatory)][string]$To, [int]$Port = 39099, [int]$Minutes = 40, [string]$Out = 'C:\ProgramData\dasio\tmp\tone-trigger.txt',
      [string]$Log = 'C:\ProgramData\dasio\instance-1\dasio.log',
      [string]$Match = 'a watched output broke|a watched test tone broke|a watched program output looped|output jumped.*peak_dbfs=(-([1-3]?[0-9]|4[0-5])|0)(\.|\b)')
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
  $ip = [Net.Dns]::GetHostAddresses($To) | Where-Object { $_.AddressFamily -eq 'InterNetwork' } | Select-Object -First 1
  if (-not $ip) { throw "cannot resolve $To to an IPv4 address" }
  "target $To = $ip" | Add-Content $Out
  $fs = New-Object IO.FileStream($Log, [IO.FileMode]::Open, [IO.FileAccess]::Read, $share)
  $pos = $fs.Length
  while ([DateTime]::UtcNow -lt $end -and -not $hit) {
    $len = $fs.Length
    if ($len -gt $pos) {
      $fs.Position = $pos; $n = $fs.Read($buf, 0, [int][Math]::Min([int64]$buf.Length, $len - $pos)); $pos += $n
      $chars = New-Object char[] ($n + 4); $cn = $dec.GetChars($buf, 0, $n, $chars, 0)
      $lines = ($carry + [string]::new($chars, 0, $cn)).Split("`n"); $carry = $lines[$lines.Count - 1]
      for ($i = 0; $i -lt $lines.Count - 1; $i++) {
        if ($lines[$i] -match $Match) { $hit = $lines[$i]; break }
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
    1..3 | ForEach-Object { [void]$u.Send($m, $m.Length, $ip.ToString(), $Port); Start-Sleep -Milliseconds 20 }
    "hit $((Get-Date).ToUniversalTime().ToString('HH:mm:ss.fff')) $hit" | Add-Content $Out
  } else { 'timeout' | Add-Content $Out }
} catch { "FAILED: $_" | Add-Content $Out
} finally { if ($fs) { $fs.Close() }; $u.Close(); 'end' | Add-Content $Out }
