# AbleSet OSC client for the dasio event soak (dasio#1). Sends to AbleSet's UDP 39051 on this PC
# and reads /getValues replies on a local port. rec-trace.ps1 drives the player through it.
#   ableset-osc.ps1 -Address /setlist/jumpToSong -Int 2
#   ableset-osc.ps1 -Address /global/play
#   ableset-osc.ps1 -Values        (prints isPlaying, activeSongName, activeSongIndex)
param([string]$Address = '', [int]$Int = [int]::MinValue, [string]$Str = '', [switch]$Values, [int]$ReplyPort = 39061)
$ErrorActionPreference = 'Stop'
function Pad([byte[]]$b) { $n = $b.Length + 1; while ($n % 4) { $n++ }; $o = New-Object byte[] $n; [Array]::Copy($b, $o, $b.Length); ,$o }
function OscMsg([string]$addr, [object[]]$args2) {
    $ms = New-Object IO.MemoryStream
    $a = Pad ([Text.Encoding]::ASCII.GetBytes($addr)); $ms.Write($a, 0, $a.Length)
    $tags = ',' + (($args2 | % { if ($_ -is [int]) { 'i' } else { 's' } }) -join '')
    $t = Pad ([Text.Encoding]::ASCII.GetBytes($tags)); $ms.Write($t, 0, $t.Length)
    foreach ($x in $args2) {
        if ($x -is [int]) { $v = [BitConverter]::GetBytes([int32]$x); [Array]::Reverse($v); $ms.Write($v, 0, 4) }
        else { $s = Pad ([Text.Encoding]::UTF8.GetBytes([string]$x)); $ms.Write($s, 0, $s.Length) }
    }
    ,$ms.ToArray()
}
function ParseOsc([byte[]]$b) {
    $i = [Array]::IndexOf($b, [byte]0); $addr = [Text.Encoding]::ASCII.GetString($b, 0, $i); $p = $i + 1; while ($p % 4) { $p++ }
    $j = [Array]::IndexOf($b, [byte]0, $p); $tags = [Text.Encoding]::ASCII.GetString($b, $p, $j - $p); $p = $j + 1; while ($p % 4) { $p++ }
    $vals = @()
    foreach ($c in $tags.Substring(1).ToCharArray()) {
        switch ($c) {
            'i' { $v = $b[$p..($p+3)]; [Array]::Reverse($v); $vals += [BitConverter]::ToInt32($v, 0); $p += 4 }
            'f' { $v = $b[$p..($p+3)]; [Array]::Reverse($v); $vals += [BitConverter]::ToSingle($v, 0); $p += 4 }
            's' { $k = [Array]::IndexOf($b, [byte]0, $p); $vals += [Text.Encoding]::UTF8.GetString($b, $p, $k - $p); $p = $k + 1; while ($p % 4) { $p++ } }
            'T' { $vals += $true } 'F' { $vals += $false }
            default { return @{ addr = $addr; vals = $vals } }
        }
    }
    @{ addr = $addr; vals = $vals }
}
function Unbundle([byte[]]$b) {
    if ($b.Length -ge 16 -and [Text.Encoding]::ASCII.GetString($b, 0, 7) -eq '#bundle') {
        $p = 16; $out = @()
        while ($p + 4 -le $b.Length) {
            $sz = $b[$p..($p+3)]; [Array]::Reverse($sz); $n = [BitConverter]::ToInt32($sz, 0); $p += 4
            $el = New-Object byte[] $n; [Array]::Copy($b, $p, $el, 0, $n); $p += $n
            $out += Unbundle $el
        }
        return $out
    }
    ,(ParseOsc $b)
}
$u = New-Object Net.Sockets.UdpClient($ReplyPort)
try {
    $dst = New-Object Net.IPEndPoint([Net.IPAddress]::Loopback, 39051)
    if ($Address) {
        $a = @(); if ($Int -ne [int]::MinValue) { $a += $Int }; if ($Str) { $a += $Str }
        $m = OscMsg $Address $a; [void]$u.Send($m, $m.Length, $dst); "sent $Address $a"
    }
    if ($Values) {
        $m = OscMsg '/getValues' @($ReplyPort); [void]$u.Send($m, $m.Length, $dst)
        $u.Client.ReceiveTimeout = 400; $want = '/global/isPlaying', '/setlist/activeSongName', '/setlist/activeSongIndex', '/global/isRecording'
        $deadline = (Get-Date).AddSeconds(3)
        while ((Get-Date) -lt $deadline) {
            try { $ep = $null; $r = $u.Receive([ref]$ep) } catch { continue }
            foreach ($o in (Unbundle $r)) { if ($want -contains $o.addr) { "$($o.addr) $($o.vals -join ' ')" } }
        }
    }
} finally { $u.Close() }
