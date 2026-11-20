# Tests of scripts/records.ps1: a status reply split by each record's own
# Version, so a script newer than the filter (or older) reads it (dasio#27).
# Windows PowerShell 5.1, as on the PCs (CI: pacer-core, Windows).
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "..\scripts\records.ps1")

$script:failed = 0
$script:checks = 0
function Check([bool]$cond, [string]$what) {
    $script:checks++
    if (-not $cond) {
        $script:failed++
        Write-Output "FAIL: $what"
    }
}
function Throws([scriptblock]$block, [string]$want, [string]$what) {
    $script:checks++
    try {
        & $block | Out-Null
        $script:failed++
        Write-Output "FAIL (no error): $what"
    } catch {
        if ($_.Exception.Message -notlike "*$want*") {
            $script:failed++
            Write-Output "FAIL ($($_.Exception.Message)): $what"
        }
    }
}

# rxd.ps1's layouts: version 2 has 11 fields, version 3 adds ParamBad.
$rxd = "Version", "IfIndex", "Enabled", "Cpu", "Deferred", "Inline", "DpcRuns", "MaxBatch", "Queued", "Dropped", "Priority", "ParamBad"
$rxdLayouts = @{ 2 = $rxd[0..10]; 3 = $rxd }
# A record of `version` with `count` fields: Version, then 100 + i.
function Record([int]$version, [int]$count) {
    $r = New-Object Int64[] $count
    $r[0] = $version
    for ($i = 1; $i -lt $count; $i++) { $r[$i] = 100 + $i }
    return , $r
}

# An old filter (v2): its records read with 11 fields, no ParamBad.
$v = [Int64[]]((Record 2 11) + (Record 2 11))
$out = @(ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd")
Check ($out.Count -eq 2) "two v2 records"
Check ($out[0].Version -eq 2 -and $out[0].IfIndex -eq 101 -and $out[0].Priority -eq 110) "v2 fields in order"
Check ($null -eq $out[0].PSObject.Properties["ParamBad"]) "v2 has no ParamBad"
Check ($out[1].Deferred -eq 104) "second v2 record from field 11 on"

# The current filter (v3).
$v = [Int64[]]((Record 3 12) + (Record 3 12))
$out = @(ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd")
Check ($out.Count -eq 2 -and $out[1].ParamBad -eq 111 -and $out[1].Version -eq 3) "two v3 records"

# Mixed versions in one reply, each by its own.
$v = [Int64[]]((Record 2 11) + (Record 3 12))
$out = @(ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd")
Check ($out.Count -eq 2 -and $out[1].ParamBad -eq 111 -and $out[0].Priority -eq 110) "v2 then v3"

# Nothing: no record (no adapter, no ring).
$out = @(ConvertTo-StatusRecords -Values (New-Object Int64[] 0) -Layouts $rxdLayouts -What "rxd")
Check ($out.Count -eq 0) "an empty reply"

# A version it does not read, a record cut short: a clear error, no loop.
$v = Record 9 12
Throws { ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd" } "version 9" "unknown version"
$v = Record 0 12
Throws { ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd" } "version 0" "version 0"
$v = [Int64[]]((Record 3 12) + (Record 3 11))
Throws { ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd" } "record 1 (version 3) is cut short" "cut short"
$v = [Int64[]](Record 3 12)
$v[0] = [Int64]::MaxValue
Throws { ConvertTo-StatusRecords -Values $v -Layouts $rxdLayouts -What "rxd" } "does not read" "a huge version"

# rxring.ps1's layouts: version 1 has 11 fields, version 2 13.
$rx = "Version", "IfIndex", "Attached", "Flags", "SlotCount", "Matches", "Matched", "Delivered", "Full", "Stolen", "TooBig", "Tagged", "BadChecksum"
$rxLayouts = @{ 1 = $rx[0..10]; 2 = $rx }
$v = [Int64[]]((Record 1 11) + (Record 2 13))
$out = @(ConvertTo-StatusRecords -Values $v -Layouts $rxLayouts -What "rxring")
Check ($out.Count -eq 2 -and $out[0].TooBig -eq 110 -and $out[1].BadChecksum -eq 112) "rx v1 then v2"
Check ($null -eq $out[0].PSObject.Properties["Tagged"]) "rx v1 has no Tagged"

# The reply's bytes: a whole number of 64-bit fields.
$bytes = New-Object byte[] 16
$bytes[0] = 3
$bytes[8] = 7
$vals = ConvertTo-StatusValues $bytes "rxd"
Check ($vals.Length -eq 2 -and $vals[0] -eq 3 -and $vals[1] -eq 7) "bytes to fields"
Check ((ConvertTo-StatusValues (New-Object byte[] 0) "rxd").Length -eq 0) "no bytes"
Throws { ConvertTo-StatusValues (New-Object byte[] 12) "rxd" } "not a whole number of fields" "12 bytes"

Write-Output "$($script:checks) checks, $($script:failed) failed"
if ($script:failed -ne 0) { exit 1 }
