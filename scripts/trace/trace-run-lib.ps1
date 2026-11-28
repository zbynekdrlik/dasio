# dasio#207: the decisions of trace-run.ps1, with no loop and no side effect, so a test can dot-source this file and run
# them against fixture logs (test-trace-run-lib.ps1, run by CI in Windows PowerShell 5.1). Copied beside trace-run.ps1.

# The lines the child wrote in ITS run: none when the file was not written since $Since (a child that failed before it
# rewrote its log leaves the last run's lines there), else the lines from the last `start` line on.
function Get-RunLines([string]$Path, [datetime]$Since) {
  if (-not (Test-Path -LiteralPath $Path)) { return @() }
  if ((Get-Item -LiteralPath $Path).LastWriteTimeUtc -lt $Since.ToUniversalTime()) { return @() }
  $lines = @(Get-Content -LiteralPath $Path); $from = 0
  if ($lines.Count -eq 0) { return @() }
  for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^start ') { $from = $i } }
  return @($lines[$from..($lines.Count - 1)])
}
function Test-Hit([string]$Path, [string]$Rx, [datetime]$Since) {
  foreach ($l in (Get-RunLines $Path $Since)) { if ($l -match $Rx) { return $true } }
  return $false
}
# Why nothing may be armed now, or '' when all is well. -FreeGB overrides the free space read from the drive (tests).
function Get-Refusal([string]$Role, [string]$Tmp, [int]$KeepSaves, [int]$MinFreeGB, [long]$FreeGB = -1) {
  if ($Role -ne 'Ring') { return '' }
  if (Test-Path -LiteralPath (Join-Path $Tmp 'event-trace.stop')) { return 'the stop flag is there (a planned probe): delete event-trace.stop to arm again' }
  $n = @(Get-ChildItem -LiteralPath $Tmp -Filter 'event-*.etl' -File -ErrorAction SilentlyContinue).Count
  if ($n -ge $KeepSaves) { return "$n saved traces (limit $KeepSaves): read them off the PC and remove them, nothing is deleted here" }
  $free = $FreeGB
  if ($free -lt 0) { $free = [Math]::Floor((New-Object IO.DriveInfo((Split-Path -Qualifier (Resolve-Path -LiteralPath $Tmp)))).AvailableFreeSpace / 1GB) }
  if ($free -lt $MinFreeGB) { return "$free GB free (limit $MinFreeGB GB)" }
  return ''
}
