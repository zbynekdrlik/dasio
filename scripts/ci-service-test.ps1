# CI (bundle job, windows-2025, administrator): the bundle installed as the PC
# installs it, run as its service, killed and brought back by the SCM, then
# removed. The device runs on the runner's routed IPv4 (the PC's own address;
# install.ps1 adds none) with the stack's test clock (no dantesync on
# a runner).
#   .\ci-service-test.ps1 -Bundle <staged bundle> -DeviceConfig <test-dante-device.toml>
param(
    [Parameter(Mandatory)][string]$Bundle,
    [Parameter(Mandatory)][string]$DeviceConfig
)
$ErrorActionPreference = "Stop"

$instance = 14
$service = "dante-asio-$instance"
$exe = Join-Path $env:ProgramFiles "dante-asio\dante-asio.exe"

function Wait-Until([scriptblock]$Condition, [int]$Seconds, [string]$What) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "$What not within $Seconds s" }
        Start-Sleep -Milliseconds 200
    }
}

function Get-ServicePid {
    (Get-CimInstance Win32_Service -Filter "Name='$service'").ProcessId
}

$route = Find-NetRoute -RemoteIPAddress 192.0.2.1 | Where-Object { $_.IPAddress } | Select-Object -First 1
$ip = $route.IPAddress
Write-Output "runner address $ip"

# The test device at 10 ms (a shared runner's scheduling), its state file in
# a writable folder.
$work = Join-Path $env:RUNNER_TEMP "dante-asio-service"
New-Item -ItemType Directory -Force $work | Out-Null
$state = (Join-Path $work "state.toml") -replace '\\', '/'
$text = (Get-Content $DeviceConfig) | Where-Object { $_ -notmatch '^state_file' } | ForEach-Object {
    if ($_ -match '^latency_ns') { 'latency_ns = 10000000' } else { $_ }
}
$text += "state_file = `"$state`""
$cfg = Join-Path $work "device.toml"
Set-Content -Path $cfg -Value $text

# Refused before anything changes: an address the PC does not have (no
# extra addresses), then a config whose ports clash with an installed
# instance's (several instances share the PC's address).
function Assert-Refused([scriptblock]$Install, [string]$Why) {
    $refused = $false
    try { & $Install } catch { $refused = $_.Exception.Message -match $Why; Write-Output "refused: $($_.Exception.Message)" }
    if (-not $refused) { throw "install was not refused for: $Why" }
    if (Get-Service -Name $service -ErrorAction SilentlyContinue) { throw "a refused install made $service" }
}
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip 192.0.2.77 -Config $cfg -Name "ci-asio-dante" -TestClock } "not one of this PC's addresses"
$other = Join-Path $env:ProgramData "dante-asio\instance-15"
New-Item -ItemType Directory -Force $other | Out-Null
Set-Content -Path (Join-Path $other "device.toml") -Value "arc_port = 4441`ncmc_port = 8800`nflows_port = 4456`nprocess_id = 1"
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } "cmc_port 8800 is taken by instance-15"
Remove-Item -Recurse -Force $other

& (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock
$svc = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($svc.StartMode -ne "Auto") { throw "start mode $($svc.StartMode), not Auto" }
if ($svc.StartName -ne "LocalSystem") { throw "account $($svc.StartName), not LocalSystem" }
$failure = (& sc.exe qfailure $service) -join "`n"
foreach ($delay in 2000, 5000, 30000) {
    if ($failure -notmatch "RESTART -- Delay = $delay") { throw "no restart after $delay ms:`n$failure" }
}
$flag = (& sc.exe qfailureflag $service) -join "`n"
if ($flag -notmatch "TRUE") { throw "no restart after an error exit:`n$flag" }

# The device drives a host on the driver: the probe through the service.
Wait-Until { Test-Path (Join-Path $env:ProgramData "dante-asio\instance-$instance\dante-asio.log") } 20 "the device's log"
Start-Sleep -Seconds 2
$probe = (& $exe probe --instance $instance --seconds 3) -join "`n"
if ($LASTEXITCODE -ne 0) { throw "probe failed ($LASTEXITCODE):`n$probe" }
Write-Output $probe
# The host must reach the SERVICE's device (session 0) from this session: a
# driver that cannot see the device free-runs and reports its block as both
# latencies; the device reports B + P in and W + its TX latency out.
if ($probe -notmatch "block (\d+) latency_in (\d+) latency_out (\d+)") { throw "no latencies in the probe's line" }
$block, $in, $out = [int]$Matches[1], [int]$Matches[2], [int]$Matches[3]
if ($in -le $block -or $out -le $block) { throw "the probe ran without the service's device (latency $in/$out at block $block)" }
if ($probe -notmatch "switches (\d+)") { throw "no switches in the probe's line" }
$switches = [int]$Matches[1]
# 3 s at the test device's block (at most 256 frames at 96 kHz): over 1000.
if ($switches -lt 1000) { throw "only $switches switches in 3 s" }

# A crash: the SCM's recovery starts it again within its 2 s delay.
$before = Get-ServicePid
Stop-Process -Id $before -Force
Wait-Until { (Get-Service $service).Status -eq "Running" -and (Get-ServicePid) -ne $before -and (Get-ServicePid) -ne 0 } 15 "a restart after the kill"
Write-Output "restarted: pid $before -> $(Get-ServicePid)"

# A second instance installed while the first runs: both share the one
# installed dante-asio.exe, which the running service holds open.
$second = 15
$work2 = Join-Path $env:RUNNER_TEMP "dante-asio-service-2"
New-Item -ItemType Directory -Force $work2 | Out-Null
$state2 = (Join-Path $work2 "state.toml") -replace '\\', '/'
$text2 = $text | Where-Object { $_ -notmatch '^state_file' } | ForEach-Object {
    if ($_ -match '^name ') { 'name = "ci-device-2"' } else { $_ }
}
$text2 += "state_file = `"$state2`""
$text2 += "arc_port = 4441", "cmc_port = 8801", "flows_port = 4456", "process_id = 1"
$cfg2 = Join-Path $work2 "device.toml"
Set-Content -Path $cfg2 -Value $text2
$first = Get-ServicePid
& (Join-Path $Bundle "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock
if ((Get-Service "dante-asio-$second").Status -ne "Running") { throw "the second instance is not running" }
if ((Get-ServicePid) -ne $first) { throw "the same binary restarted the first instance: pid $first -> $(Get-ServicePid)" }
Write-Output "second instance installed beside the first (pid $first kept)"

# A new binary while both run: every running instance stops for the copy and
# runs the new binary after it.
$newer = Join-Path $env:RUNNER_TEMP "bundle-newer"
Copy-Item -Recurse -Force $Bundle $newer
# One byte past the image: a different file Windows still runs.
[System.IO.File]::AppendAllText((Join-Path $newer "dante-asio.exe"), " ")
$sums = Get-Content (Join-Path $newer "SHA256SUMS") | ForEach-Object {
    $hash, $file = $_ -split "  ", 2
    "$((Get-FileHash (Join-Path $newer $file) -Algorithm SHA256).Hash.ToLower())  $file"
}
Set-Content -Path (Join-Path $newer "SHA256SUMS") -Value $sums
& (Join-Path $newer "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock
$want = (Get-FileHash (Join-Path $newer "dante-asio.exe")).Hash
if ((Get-FileHash $exe).Hash -ne $want) { throw "the installed binary is not the new one" }
foreach ($n in $instance, $second) {
    if ((Get-Service "dante-asio-$n").Status -ne "Running") { throw "dante-asio-$n is not running after the update" }
}
if ((Get-ServicePid) -eq $first) { throw "the first instance still runs the old binary (pid $first)" }
Write-Output "both instances run the new binary"
& (Join-Path $newer "uninstall.ps1") -Instance $second -Ip $ip -Name "ci-asio-dante-2"
Wait-Until { -not (Get-Service -Name "dante-asio-$second" -ErrorAction SilentlyContinue) } 15 "the second service's removal"

& (Join-Path $Bundle "uninstall.ps1") -Instance $instance -Ip $ip -Name "ci-asio-dante"
Wait-Until { -not (Get-Service -Name $service -ErrorAction SilentlyContinue) } 15 "the service's removal"
if (-not (Get-NetIPAddress -IPAddress $ip -ErrorAction SilentlyContinue)) { throw "uninstall removed the runner's own address" }
if (Test-Path "HKLM:\SOFTWARE\ASIO\ci-asio-dante") { throw "the driver's ASIO entry is still there" }
Get-Content (Join-Path $env:ProgramData "dante-asio\instance-$instance\dante-asio.log") | Select-Object -Last 15
Write-Output "service test passed"
