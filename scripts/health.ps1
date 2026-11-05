# dasio PC health, part by part (dasio#1). Samples one window and prints one
# line per part, so a change can be judged by what it did to each part, not
# only by the overall late count. Read only: it changes nothing.
#
#   health.ps1 -Seconds 120 -Groups "os=0-3;dasio=8-15" `
#              -Procs "Ableton Live 12 Suite" -PacerScripts C:\dasio-runs\probe `
#              -Out C:\dasio-runs\health-<label>.txt
#
# -Groups names the PC's CPU groups (site data: it lives in the PC's own
# call, never here). DPC and interrupt time are given for every group.
param(
    [ValidateRange(10, 3600)][int]$Seconds = 120,
    [Parameter(Mandatory)][string]$Groups,
    [string]$Procs = "",
    [string]$Root = "C:\ProgramData\dasio",
    [string]$PacerScripts = "",
    [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = "Stop"

function Cpus([string]$spec) {
    $set = @()
    foreach ($part in $spec.Split(",")) {
        if ($part -match '^(\d+)-(\d+)$') { $set += [int]$matches[1]..[int]$matches[2] }
        elseif ($part -match '^\d+$') { $set += [int]$part }
        else { throw "bad CPU list '$spec'" }
    }
    return $set
}

$groupList = foreach ($g in $Groups.Split(";")) {
    if ($g -notmatch '^([^=]+)=(.+)$') { throw "bad group '$g' (name=cpus)" }
    [pscustomobject]@{ Name = $matches[1]; Cpus = Cpus $matches[2] }
}
$procNames = @($Procs.Split(";") | Where-Object { $_ })

# A status field that is missing (a renamed field in ktx.ps1 / rxd.ps1)
# stops the run instead of reading as 0.
function Field($obj, [string]$name, [string]$script) {
    $v = $obj.$name
    if ($null -eq $v) { throw "$script status has no field $name" }
    return [int64]$v
}

function PacerStatus {
    if (-not $PacerScripts) { return $null }
    $k = & (Join-Path $PacerScripts "ktx.ps1") status
    $r = & (Join-Path $PacerScripts "rxd.ps1") status
    return [pscustomobject]@{
        TxNoSlot = Field $k TxNoSlot ktx; Failed = Field $k Failed ktx; TxFrames = Field $k TxFrames ktx
        Deferred = Field $r Deferred rxd; DpcRuns = Field $r DpcRuns rxd; MaxBatch = Field $r MaxBatch rxd
    }
}

# CPU time per process id, with its start time: a process that exits or
# restarts in the window is reported, never counted as a negative delta.
function ProcCpu {
    $h = @{}
    foreach ($n in $procNames) {
        $h[$n] = @{}
        foreach ($p in @(Get-Process -Name $n -ErrorAction SilentlyContinue)) {
            # A protected process can refuse these reads: it is left out
            # and reported, not allowed to stop the run.
            try {
                $h[$n][$p.Id] = [pscustomobject]@{ Seconds = $p.TotalProcessorTime.TotalSeconds; Start = $p.StartTime }
            } catch {
                $h[$n][$p.Id] = $null
            }
        }
    }
    return $h
}

# The output folder must exist before the window, not after it.
$outDir = Split-Path -Parent $Out
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Force $outDir | Out-Null }
# So must the instances' folder: a wrong -Root fails now, not after the window.
if (-not (Test-Path $Root)) { throw "no folder $Root (-Root)" }

# The window's lines of one instance's log, from dasio.log.1 and dasio.log.
# The log rotates at 10 MB (dasio.log -> dasio.log.1). A rotation while
# the files are read can hide or move lines, so then both are read once
# more and the passes are merged; a line carries a microsecond time stamp,
# so a line read twice counts once. Files are streamed with FileShare
# ReadWrite + Delete (the service may write, rename or delete them
# meanwhile) and lines compare ordinally (ISO time stamps).
function ReadWindow([string]$folder, [string]$from, [string]$toEnd) {
    $log = Join-Path $folder 'dasio.log'
    $old = Join-Path $folder 'dasio.log.1'
    $share = [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    $seen = New-Object 'System.Collections.Generic.HashSet[string]'
    $lines = New-Object System.Collections.Generic.List[string]
    $rotated = $false
    for ($pass = 0; $pass -lt 2; $pass++) {
        $born = if (Test-Path $log) { (Get-Item $log).CreationTimeUtc } else { $null }
        $missed = $false
        foreach ($f in @($old, $log)) {
            if (-not (Test-Path $f)) { continue }
            try {
                $fs = New-Object System.IO.FileStream($f, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, $share)
            } catch [System.IO.FileNotFoundException] {
                $missed = $true; continue
            }
            $sr = New-Object System.IO.StreamReader($fs)
            try {
                while ($null -ne ($ln = $sr.ReadLine())) {
                    if ([string]::CompareOrdinal($ln, $from) -ge 0 -and [string]::CompareOrdinal($ln, $toEnd) -le 0 -and $seen.Add($ln)) {
                        $lines.Add($ln)
                    }
                }
            } finally { $sr.Dispose() }
        }
        $after = if (Test-Path $log) { (Get-Item $log).CreationTimeUtc } else { $null }
        if (-not $missed -and $born -eq $after) { break }
        $rotated = $true
    }
    return [pscustomobject]@{ Lines = $lines; Rotated = $rotated }
}

$startUtc = (Get-Date).ToUniversalTime()
$pacer0 = PacerStatus
$proc0 = ProcCpu
$counters = @('\Processor(*)\% Processor Time', '\Processor(*)\% DPC Time', '\Processor(*)\% Interrupt Time',
    '\Network Interface(*)\Bytes Received/sec', '\Network Interface(*)\Bytes Sent/sec')
$samples = Get-Counter -Counter $counters -SampleInterval 1 -MaxSamples $Seconds
$proc1 = ProcCpu
$pacer1 = PacerStatus
$endUtc = (Get-Date).ToUniversalTime()

$elapsed = ($endUtc - $startUtc).TotalSeconds
$lines = @("health $($startUtc.ToString('yyyy-MM-ddTHH:mm:ssZ')) - $($endUtc.ToString('HH:mm:ssZ')) ($Seconds s)")

# CPU groups: average and peak of the per-second group average.
$all = foreach ($s in $samples) { $s.CounterSamples }
foreach ($kind in @('% processor time', '% dpc time', '% interrupt time')) {
    foreach ($g in $groupList) {
        $names = @($g.Cpus | ForEach-Object { "$_" } | Select-Object -Unique)
        $found = @($all | Where-Object { $_.Path -like "*\$kind" -and $names -contains $_.InstanceName } |
            Select-Object -ExpandProperty InstanceName -Unique)
        if ($found.Count -ne $names.Count) {
            throw "group $($g.Name): the counters expose $($found.Count) of its $($names.Count) CPUs"
        }
        $per = $all | Where-Object { $_.Path -like "*\$kind" -and $names -contains $_.InstanceName } |
            Group-Object Timestamp | ForEach-Object { ($_.Group.CookedValue | Measure-Object -Average).Average }
        $avg = ($per | Measure-Object -Average).Average
        $max = ($per | Measure-Object -Maximum).Maximum
        $peakCpu = $all | Where-Object { $_.Path -like "*\$kind" -and $names -contains $_.InstanceName } |
            Sort-Object CookedValue -Descending | Select-Object -First 1
        $lines += ('cpu {0,-14} {1,-17} avg {2,5:N1} %  peak-sec {3,5:N1} %  worst cpu{4} {5,5:N1} %' -f
            $g.Name, $kind.TrimStart('% '), $avg, $max, $peakCpu.InstanceName, $peakCpu.CookedValue)
    }
}

# NIC bytes.
foreach ($dir in @('bytes received/sec', 'bytes sent/sec')) {
    $nic = $all | Where-Object { $_.Path -like "*\$dir" -and $_.InstanceName -notmatch 'isatap|teredo|loopback' }
    foreach ($n in ($nic | Group-Object InstanceName)) {
        $v = $n.Group.CookedValue
        $lines += ('nic {0,-40} {1,-18} avg {2,8:N2} MB/s  peak {3,8:N2} MB/s' -f
            $n.Name, $dir, (($v | Measure-Object -Average).Average / 1e6), (($v | Measure-Object -Maximum).Maximum / 1e6))
    }
}

# Processes: CPU time used in the window, as % of one CPU.
foreach ($n in $procNames) {
    $used = 0.0; $gone = 0; $new = 0; $unread = 0
    foreach ($id in $proc0[$n].Keys) {
        $a = $proc0[$n][$id]; $b = $proc1[$n][$id]
        if ($null -eq $a -or ($proc1[$n].ContainsKey($id) -and $null -eq $b)) { $unread++ }
        elseif ($null -eq $b) { $gone++ }
        elseif ($b.Start -ne $a.Start) { $gone++; $new++ }   # the id was reused by a new process
        else { $used += $b.Seconds - $a.Seconds }
    }
    foreach ($id in $proc1[$n].Keys) { if (-not $proc0[$n].ContainsKey($id)) { $new++ } }
    $note = if ($gone -or $new -or $unread) {
        "  (in the window: $gone exited, $new started, $unread unreadable; only the ones alive and readable throughout are counted)"
    } else { '' }
    $lines += ('proc {0,-30} {1,6:N1} % of one CPU{2}' -f $n, ($used / $elapsed * 100), $note)
}

# The NDIS filter's counters over the window.
if ($pacer0) {
    $lines += ('pacer TxNoSlot +{0}  Failed +{1}  TxFrames +{2}  rxd Deferred +{3}  DpcRuns +{4}  MaxBatch {5}' -f
        ($pacer1.TxNoSlot - $pacer0.TxNoSlot), ($pacer1.Failed - $pacer0.Failed), ($pacer1.TxFrames - $pacer0.TxFrames),
        ($pacer1.Deferred - $pacer0.Deferred), ($pacer1.DpcRuns - $pacer0.DpcRuns), $pacer1.MaxBatch)
}

# Each dasio instance's log over the window.
$from = $startUtc.ToString('yyyy-MM-ddTHH:mm:ss')
$to = $endUtc.ToString('yyyy-MM-ddTHH:mm:ss')
$toEnd = "$to~"
foreach ($dir in Get-ChildItem $Root -Directory -Filter 'instance-*') {
    $log = Join-Path $dir.FullName 'dasio.log'
    if (-not (Test-Path $log) -and -not (Test-Path (Join-Path $dir.FullName 'dasio.log.1'))) { continue }
    $read = ReadWindow $dir.FullName $from $toEnd
    $w = $read.Lines
    $rotNote = if ($read.Rotated) { '  (the log rotated while read: re-read and merged)' } else { '' }
    $restartSec = @($w | Select-String 'TX schedule starts again' | ForEach-Object { $_.Line.Substring(0, 19) } | Sort-Object -Unique).Count
    $failed = 0; $w | Select-String 'TX sends failed after' | ForEach-Object { if ($_.Line -match 'errors=(\d+)') { $failed += [int]$matches[1] } }
    $lateSec = @($w | Select-String 'TX packets older' | ForEach-Object { $_.Line.Substring(0, 19) } | Sort-Object -Unique).Count
    $ov = 0; $ms = 0; $pumpSec = 0
    $w | Select-String 'engine pump lost or overran' | ForEach-Object {
        $pumpSec++
        if ($_.Line -match 'missed=(\d+) overruns=(\d+)') { $ms += [int]$matches[1]; $ov += [int]$matches[2] }
    }
    $rxGaps = @($w | Select-String 'RX packets missing').Count
    $hold = @($w | Select-String 'holdover: dsync').Count
    $maxTurn = 0; $p99 = 0
    $w | Select-String 'DAW turnaround' | ForEach-Object {
        if ($_.Line -match 'p99_us=(\d+) max_us=(\d+)') { $p99 = [Math]::Max($p99, [int]$matches[1]); $maxTurn = [Math]::Max($maxTurn, [int]$matches[2]) }
    }
    # dasio logs the DAW turnaround every ~10 s: 'p99' is the highest of
    # those reports' p99s, 'max' the highest single block.
    $lines += ('dasio {0,-11} TX restart-sec {1}  TX failed {2}  late-sec {3}  pump-sec {4} overruns {5} missed {6}  daw worst-report-p99 {7} us max {8} us  rx-gap lines {9}  holdover {10}{11}' -f
        $dir.Name, $restartSec, $failed, $lateSec, $pumpSec, $ov, $ms, $p99, $maxTurn, $rxGaps, $hold, $rotNote)
}

$lines | Set-Content $Out
$lines
