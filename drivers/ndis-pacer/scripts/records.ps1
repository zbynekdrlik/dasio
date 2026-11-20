# dasio NDIS filter: a status reply split into records by each record's own
# Version (its first field), so a script reads a filter that is older or
# newer than itself: a v2 rxd record holds 11 fields, a v3 one 12. Dot-
# sourced by rxd.ps1 and rxring.ps1 (tests/test_records.ps1 tests it).
#   $Values  the reply as 64-bit fields
#   $Layouts version -> the field names of a record of that version
#   $What    the reply's name, for the error

function ConvertTo-StatusRecords {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][Int64[]]$Values,
        [Parameter(Mandatory)][hashtable]$Layouts,
        [Parameter(Mandatory)][string]$What
    )
    $at = 0
    # Bounded by the field count: every record takes at least one field.
    for ($n = 0; $n -lt $Values.Length -and $at -lt $Values.Length; $n++) {
        $version = $Values[$at]
        $names = $null
        if ($version -ge 1 -and $version -le 1000) { $names = $Layouts[[int]$version] }
        if (-not $names) { throw "$What status record $n has version $version, which this script does not read" }
        if ($at + $names.Count -gt $Values.Length) { throw "$What status record $n (version $version) is cut short" }
        $o = [ordered]@{}
        for ($j = 0; $j -lt $names.Count; $j++) { $o[$names[$j]] = $Values[$at + $j] }
        [pscustomobject]$o
        $at += $names.Count
    }
}

# The reply's bytes as 64-bit fields (a whole number of them).
function ConvertTo-StatusValues([byte[]]$Raw, [string]$What) {
    if ($Raw.Length % 8 -ne 0) { throw "$What status of $($Raw.Length) bytes is not a whole number of fields" }
    $v = New-Object Int64[] ($Raw.Length / 8)
    [Buffer]::BlockCopy($Raw, 0, $v, 0, $Raw.Length)
    return , $v
}
