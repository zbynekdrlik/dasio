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
$service = "dasio-$instance"
$exe = Join-Path $env:ProgramFiles "dasio\dasio.exe"

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
$work = Join-Path $env:RUNNER_TEMP "dasio-service"
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
$other = Join-Path $env:ProgramData "dasio\instance-15"
New-Item -ItemType Directory -Force $other | Out-Null
Set-Content -Path (Join-Path $other "device.toml") -Value "arc_port = 4441`ncmc_port = 8800`nflows_port = 4456`nprocess_id = 1"
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } "cmc_port 8800 is taken by instance-15"
Remove-Item -Recurse -Force $other

# A state file another device left at this config's state path (1.11.2026,
# FOH PC: the new config came up under the old device's name with its
# subscriptions): the install moves it aside, the device starts as the
# config names it.
$stateWin = $state -replace '/', '\'
Set-Content -Path $stateWin -Value "name = `"foreign-device`"`nconfig_name = `"foreign-device`""

# The product's former name (dasio#1, 3.11.2026): its service of this
# instance goes and its state files come over.
$formerData = Join-Path $env:ProgramData "dante-asio\instance-$instance"
New-Item -ItemType Directory -Force $formerData | Out-Null
Set-Content -Path (Join-Path $formerData "state-former.toml") -Value "name = `"former-device`""
New-Service -Name "dante-asio-$instance" -BinaryPathName "`"$env:SystemRoot\System32\svchost.exe`"" -StartupType Manual | Out-Null

# A failed native command just before (FOH 3.11.2026: the install stopped
# the band's service and threw, as dtray.exe, a GUI program, leaves the
# exit code of whatever ran before it).
cmd /c exit 7
& (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock
if (Get-Service -Name "dante-asio-$instance" -ErrorAction SilentlyContinue) { throw "the former service is still there" }
if (-not (Test-Path (Join-Path $env:ProgramData "dasio\instance-$instance\state-former.toml"))) { throw "the former state file did not come over" }
Remove-Item -Recurse -Force (Join-Path $env:ProgramData "dante-asio")
Write-Output "the former service went and its state came over"
# The instance runs its own binary, named after its device.
$own = Join-Path $env:ProgramFiles "dasio\dasio-test-device.exe"
if ((Get-CimInstance Win32_Service -Filter "Name='$service'").PathName -notmatch [regex]::Escape("`"$own`"")) { throw "$service does not run $own" }
$aside = @(Get-ChildItem $work -Filter "state.toml.replaced-*")
if ($aside.Count -ne 1) { throw "the foreign state was not moved aside ($($aside.Count) copies)" }
if ((Get-Content $aside[0].FullName -Raw) -notmatch 'foreign-device') { throw "the moved-aside file is not the foreign state" }
# The device writes its state only when something in it changes, so none
# may be there yet; one that is there is its own.
if ((Test-Path $stateWin) -and ((Get-Content $stateWin -Raw) -match 'foreign-device')) { throw "the device runs under the foreign state" }
Write-Output "a foreign state file was moved aside: $($aside[0].Name)"
# dtray (dstack's tray): installed, started at the user's logon, the instance
# expected, and its line read from the running device's status.
$dtray = Join-Path $env:ProgramFiles "dasio\dtray.exe"
if (-not (Test-Path $dtray)) { throw "no dtray installed" }
$runKey = "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"
if (-not (Get-ItemProperty $runKey -Name "dtray" -ErrorAction SilentlyContinue)) { throw "dtray does not start at logon" }
if (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -Name "dasio-tray" -ErrorAction SilentlyContinue) { throw "the former dasio-tray still starts at logon" }
$expected = Join-Path $env:ProgramData "dstack\devices\test-device.device"
if (-not (Test-Path $expected)) { throw "no expected-device file $expected" }
Wait-Until { (& $dtray --print 2>$null | Out-String) -match "(no DAW|DAW attached) .+ @ \d+ kHz .+ in [\d.]+ / out [\d.]+ ms .+ late \d+/min" } 30 "dtray's dasio line for the instance"
$trayOut = & $dtray --print 2>$null | Out-String
Write-Output "dtray:`n$trayOut"
if ($trayOut -notmatch "test-device") { throw "dtray does not name the device" }
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
Wait-Until { Test-Path (Join-Path $env:ProgramData "dasio\instance-$instance\dasio.log") } 20 "the device's log"
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

# A second instance installed while the first runs: each runs its own
# binary, so the first keeps running.
$second = 15
$work2 = Join-Path $env:RUNNER_TEMP "dasio-service-2"
New-Item -ItemType Directory -Force $work2 | Out-Null
$state2 = (Join-Path $work2 "state.toml") -replace '\\', '/'
$text2 = $text | Where-Object { $_ -notmatch '^state_file' } | ForEach-Object {
    if ($_ -match '^name ') { 'name = "dasio-ci-2"' } else { $_ }
}
$text2 += "state_file = `"$state2`""
$text2 += "arc_port = 4441", "cmc_port = 8801", "flows_port = 4456", "process_id = 1"
$cfg2 = Join-Path $work2 "device.toml"
Set-Content -Path $cfg2 -Value $text2
$first = Get-ServicePid
& (Join-Path $Bundle "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock -SpareBlocks 1 -KernelTx
if ((Get-Service "dasio-$second").Status -ne "Running") { throw "the second instance is not running" }
if ((Get-CimInstance Win32_Service -Filter "Name='dasio-$second'").PathName -notmatch '--kernel-tx') { throw "the second instance runs without --kernel-tx" }
# The runner has no dasio_pacer: kernel TX falls back to the UDP sockets
# and says so, and the instance serves on.
Wait-Until { (Get-Content (Join-Path $env:ProgramData "dasio\instance-$second\dasio.log") -ErrorAction SilentlyContinue) -match "audio TX stays on the UDP sockets: dasio's NDIS filter could not be opened" } 20 "the second instance's kernel TX fallback"
if ((Get-CimInstance Win32_Service -Filter "Name='dasio-$second'").PathName -notmatch '--spare-blocks 1') { throw "the second instance runs without its spare block" }
Wait-Until { (Get-Content (Join-Path $env:ProgramData "dasio\instance-$second\dasio.log") -ErrorAction SilentlyContinue) -match 'spare_blocks=1' } 20 "the second instance serving with a spare block"
if ((Get-ServicePid) -ne $first) { throw "the same binary restarted the first instance: pid $first -> $(Get-ServicePid)" }
Write-Output "second instance installed beside the first (pid $first kept)"

# A new binary for the second instance while both run: only it restarts;
# the first runs its own binary until its own install.
$newer = Join-Path $env:RUNNER_TEMP "bundle-newer"
Copy-Item -Recurse -Force $Bundle $newer
# One byte past the image: a different file Windows still runs.
[System.IO.File]::AppendAllText((Join-Path $newer "dasio.exe"), " ")
$sums = Get-Content (Join-Path $newer "SHA256SUMS") | ForEach-Object {
    $hash, $file = $_ -split "  ", 2
    "$((Get-FileHash (Join-Path $newer $file) -Algorithm SHA256).Hash.ToLower())  $file"
}
Set-Content -Path (Join-Path $newer "SHA256SUMS") -Value $sums
& (Join-Path $newer "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock
$want = (Get-FileHash (Join-Path $newer "dasio.exe")).Hash
# A device already named dasio-*: its binary keeps the name (no doubled prefix).
$own2 = Join-Path $env:ProgramFiles "dasio\dasio-ci-2.exe"
if ((Get-FileHash $exe).Hash -ne $want) { throw "the installed binary is not the new one" }
if ((Get-FileHash $own2).Hash -ne $want) { throw "the second instance's binary is not the new one" }
if ((Get-FileHash $own).Hash -eq $want) { throw "the first instance's binary changed without its install" }
if ((Get-ServicePid) -ne $first) { throw "the second instance's update restarted the first (pid $first -> $(Get-ServicePid))" }
& (Join-Path $newer "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock
if ((Get-FileHash $own).Hash -ne $want) { throw "the first instance's binary is not the new one after its install" }
foreach ($n in $instance, $second) {
    if ((Get-Service "dasio-$n").Status -ne "Running") { throw "dasio-$n is not running after the update" }
}
if ((Get-ServicePid) -eq $first) { throw "the first instance still runs the old binary (pid $first)" }
Write-Output "each instance runs the new binary after its own install"

# Updates back and forth, each instance in turn, as an update of the PC
# runs them: each new binary pauses the other instance, and the device
# managing the address stops while the other is a member. No instance may
# end on its own (1.11.2026, iem-snv: with dstack v0.4.21 every such
# replay ended the member with os error 10054 and the SCM restarted it).
$mark = Get-Date
foreach ($round in 1..3) {
    foreach ($from in $Bundle, $newer) {
        & (Join-Path $from "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock | Out-Null
        & (Join-Path $from "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock | Out-Null
    }
}
Start-Sleep -Seconds 5
$exits = @(Get-WinEvent -FilterHashtable @{ LogName = "System"; ProviderName = "Service Control Manager"; StartTime = $mark } -ErrorAction SilentlyContinue |
    Where-Object { $_.Id -in 7024, 7031, 7034 -and $_.Message -match "dasio instance" })
if ($exits.Count -gt 0) {
    $exits | ForEach-Object { Write-Output "$($_.TimeCreated) $($_.Id) $($_.Message)" }
    throw "$($exits.Count) service error exits during the updates"
}
foreach ($n in $instance, $second) {
    if ((Get-Service "dasio-$n").Status -ne "Running") { throw "dasio-$n is not running after the updates" }
}
Write-Output "six updates in turn: no instance ended on its own"
# A DAW holds the installed driver (it keeps it loaded until it quits): a
# new driver is refused before anything stops, or kept out with
# -KeepHeldDriver.
$newest = Join-Path $env:RUNNER_TEMP "bundle-newest"
Copy-Item -Recurse -Force $newer $newest
[System.IO.File]::AppendAllText((Join-Path $newest "dasio_driver.dll"), " ")
$sums = Get-Content (Join-Path $newest "SHA256SUMS") | ForEach-Object {
    $hash, $file = $_ -split "  ", 2
    "$((Get-FileHash (Join-Path $newest $file) -Algorithm SHA256).Hash.ToLower())  $file"
}
Set-Content -Path (Join-Path $newest "SHA256SUMS") -Value $sums
$dll = Join-Path $env:ProgramFiles "dasio\dasio_driver.dll"
$held = [System.IO.File]::Open($dll, "Open", "Read", "Read")
try {
    $pid2 = (Get-CimInstance Win32_Service -Filter "Name='dasio-$second'").ProcessId
    $refused = $false
    try { & (Join-Path $newest "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock } catch { $refused = $_.Exception.Message -match "holds" }
    if (-not $refused) { throw "a new driver over a held one was not refused" }
    if ((Get-CimInstance Win32_Service -Filter "Name='dasio-$second'").ProcessId -ne $pid2) { throw "the refused install stopped dasio-$second" }
    & (Join-Path $newest "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock -KeepHeldDriver
    if ((Get-FileHash $dll).Hash -ne (Get-FileHash (Join-Path $newer "dasio_driver.dll")).Hash) { throw "the held driver was replaced" }
} finally {
    $held.Close()
}
Write-Output "a held driver: refused, or kept with -KeepHeldDriver"

& (Join-Path $newer "uninstall.ps1") -Instance $second -Ip $ip -Name "ci-asio-dante-2"
Wait-Until { -not (Get-Service -Name "dasio-$second" -ErrorAction SilentlyContinue) } 15 "the second service's removal"
if (Test-Path $own2) { throw "uninstall left the second instance's binary" }

cmd /c exit 7
& (Join-Path $Bundle "uninstall.ps1") -Instance $instance -Ip $ip -Name "ci-asio-dante"
Wait-Until { -not (Get-Service -Name $service -ErrorAction SilentlyContinue) } 15 "the service's removal"
if (-not (Get-NetIPAddress -IPAddress $ip -ErrorAction SilentlyContinue)) { throw "uninstall removed the runner's own address" }
if (Test-Path "HKLM:\SOFTWARE\ASIO\ci-asio-dante") { throw "the driver's ASIO entry is still there" }
if (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -Name "dasio-tray" -ErrorAction SilentlyContinue) { throw "the tray still starts at logon with no instance left" }
if (Test-Path $expected) { throw "uninstall left the expected-device file" }
if (Get-ItemProperty $runKey -Name "dtray" -ErrorAction SilentlyContinue) { throw "dtray still starts at logon with no instance left" }
Get-Content (Join-Path $env:ProgramData "dasio\instance-$instance\dasio.log") | Select-Object -Last 15
Write-Output "service test passed"
