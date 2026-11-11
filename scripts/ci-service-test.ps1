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

# The bundle and the configs as the PC keeps them: in a folder only SYSTEM
# and administrators may change, users read (install.ps1 refuses any other,
# dasio#28 E19).
function New-AdminOnlyFolder([string]$Path) {
    New-Item -ItemType Directory -Force $Path | Out-Null
    & icacls.exe $Path /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls $Path failed ($LASTEXITCODE)" }
}
$data = Join-Path $env:ProgramData "dasio"
$bundles = Join-Path $data "bundles"
$staged = Join-Path $bundles "ci"
New-AdminOnlyFolder $staged
Copy-Item -Force (Join-Path $Bundle "*") $staged
$Bundle = $staged

$route = Find-NetRoute -RemoteIPAddress 192.0.2.1 | Where-Object { $_.IPAddress } | Select-Object -First 1
$ip = $route.IPAddress
# The first instance is installed by its interface's name (-Iface, dasio#29):
# the device takes that interface's one IPv4 address at its start.
$iface = $route.InterfaceAlias
$ifaceAddrs = @(Get-NetIPAddress -InterfaceAlias $iface -AddressFamily IPv4)
if ($ifaceAddrs.Count -ne 1 -or $ifaceAddrs[0].IPAddress -ne $ip) { throw "the runner's interface '$iface' does not hold exactly ${ip}: $($ifaceAddrs.IPAddress -join ', ')" }
Write-Output "runner address $ip on '$iface'"

# The test device at 10 ms (a shared runner's scheduling), its config and
# state file in an admin-only folder.
$work = Join-Path $bundles "ci-service"
New-AdminOnlyFolder $work
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
# -Ip or -Iface: exactly one; an interface without exactly one IPv4 address.
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Iface $iface -Config $cfg -Name "ci-asio-dante" -TestClock } "exactly one of -Ip <address> and -Iface"
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Config $cfg -Name "ci-asio-dante" -TestClock } "exactly one of -Ip <address> and -Iface"
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Iface "dasio-no-such-interface" -Config $cfg -Name "ci-asio-dante" -TestClock } "has 0 IPv4 addresses"

# Elevated, the install runs only what non-administrators cannot change
# (dasio#28 E19). Refused before anything changes: a bundle in a folder
# users may change (a folder made directly under C:\ inherits Authenticated
# Users: Modify, as FOH's C:\dasio-runs did), a config in one, and an
# admin-only bundle inside a folder users may delete (renamed away, a
# folder of their own takes its place).
$open = "C:\dasio-ci-open"
New-Item -ItemType Directory -Force $open | Out-Null
& icacls.exe $open /grant "*S-1-5-32-545:(OI)(CI)M" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls $open failed ($LASTEXITCODE)" }
$openBundle = Join-Path $open "bundle"
New-Item -ItemType Directory -Force $openBundle | Out-Null
Copy-Item -Force (Join-Path $Bundle "*") $openBundle
Assert-Refused { & (Join-Path $openBundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ("the bundle folder .+" + [regex]::Escape("BUILTIN\Users (on $openBundle)"))
$openCfg = Join-Path $open "device.toml"
Copy-Item -Force $cfg $openCfg
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $openCfg -Name "ci-asio-dante" -TestClock } "the config .+ can be changed by BUILTIN\\Users"
$inner = Join-Path $open "inner"
New-AdminOnlyFolder $inner
Copy-Item -Force (Join-Path $Bundle "*") $inner
Assert-Refused { & (Join-Path $inner "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ([regex]::Escape("BUILTIN\Users (on $open)"))
Remove-Item -Recurse -Force $open
# A path through a link: the checks would read the target's permissions,
# never those of the folders above the target.
$viaLink = Join-Path $env:ProgramData "dasio-ci-via-link"
New-Item -ItemType Junction -Path $viaLink -Target $Bundle | Out-Null
Assert-Refused { & (Join-Path $viaLink "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ([regex]::Escape("goes through a link ($viaLink)"))
[System.IO.Directory]::Delete($viaLink)
# dasio's data folder becomes admin-only; refused with nothing else changed:
# a folder in it users may change (a rollback folder made before the
# install: the exe the install keeps there goes back into Program Files on
# a failure, and the LocalSystem service runs it), and a link in it (this
# elevated script would follow it).
$planted = Join-Path $data "instance-$instance\rollback"
New-Item -ItemType Directory -Force $planted | Out-Null
& icacls.exe $planted /grant "*S-1-5-32-545:(OI)(CI)M" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls $planted failed ($LASTEXITCODE)" }
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ([regex]::Escape("BUILTIN\Users (on $planted)"))
if ((Get-Acl $data).AreAccessRulesProtected) { throw "the refused install changed the permissions of $data" }
Remove-Item -Recurse -Force $planted
# An entry a folder in it only passes down (inherit-only): its items carry it
# as inherited, and protecting the data folder would not remove it.
$passer = Join-Path $data "instance-$instance\passer"
New-Item -ItemType Directory -Force $passer | Out-Null
& icacls.exe $passer /grant "*S-1-5-32-545:(OI)(CI)(IO)M" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls $passer failed ($LASTEXITCODE)" }
Set-Content -Path (Join-Path $passer "device.toml") -Value "x"
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ([regex]::Escape("BUILTIN\Users (on $passer)"))
if ((Get-Acl $data).AreAccessRulesProtected) { throw "the refused install changed the permissions of $data" }
Remove-Item -Recurse -Force $passer
$link = Join-Path $data "instance-$instance\link"
New-Item -ItemType Junction -Path $link -Target $env:TEMP | Out-Null
Assert-Refused { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } ([regex]::Escape("links under $data"))
# The junction only, never what it points to.
[System.IO.Directory]::Delete($link)
Write-Output "refused: a bundle, a config or a data folder others may change, and a link in the data folder"

# A state file another device left at this config's state path (1.11.2026,
# FOH PC: the new config came up under the old device's name with its
# subscriptions): the install moves it aside, the device starts as the
# config names it.
$stateWin = $state -replace '/', '\'
Set-Content -Path $stateWin -Value "name = `"foreign-device`"`nconfig_name = `"foreign-device`""

# A failed native command just before (FOH 3.11.2026: the install stopped
# the band's service and threw, as dtray.exe, a GUI program, leaves the
# exit code of whatever ran before it).
cmd /c exit 7
& (Join-Path $Bundle "install.ps1") -Instance $instance -Iface $iface -Config $cfg -Name "ci-asio-dante" -TestClock
# dasio's data folder: protected from what ProgramData passes down (users
# may add files and folders there); users read it and change nothing in it.
$rootAcl = Get-Acl $data
if (-not $rootAcl.AreAccessRulesProtected) { throw "$data still inherits ProgramData's permissions" }
$others = @("S-1-5-32-545", "S-1-5-11", "S-1-1-0", "S-1-5-4")
foreach ($path in $data, (Join-Path $data "instance-$instance"), (Join-Path $data "instance-$instance\device.toml")) {
    foreach ($rule in (Get-Acl $path).GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq "Allow" -and $others -contains $rule.IdentityReference.Value -and ([int]$rule.FileSystemRights -band 0x500D0046)) {
            throw "$($rule.IdentityReference.Value) may change $path ($($rule.FileSystemRights))"
        }
    }
}
Write-Output "the data folder is admin-only, users read"
# The instance runs its own binary, named after its device.
$own = Join-Path $env:ProgramFiles "dasio\dasio-test-device.exe"
if ((Get-CimInstance Win32_Service -Filter "Name='$service'").PathName -notmatch [regex]::Escape("`"$own`"")) { throw "$service does not run $own" }
if ((Get-CimInstance Win32_Service -Filter "Name='$service'").PathName -notmatch [regex]::Escape("--iface `"$iface`"")) { throw "$service does not run on its interface's name" }
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
# On its interface's address, its control ports the first free slot (its
# config names none, dasio#89; nothing else holds slot 0 on the runner).
Wait-Until { (Get-Content (Join-Path $env:ProgramData "dasio\instance-$instance\dasio.log") -ErrorAction SilentlyContinue) -match "the device's address ip=$([regex]::Escape($ip)) place=" } 20 "the device on its interface's address"
Wait-Until { (Get-Content (Join-Path $env:ProgramData "dasio\instance-$instance\dasio.log") -ErrorAction SilentlyContinue) -match "the device's control ports: the first slot free on its address arc=4440 cmc=8800 flows=4455 process_id=0" } 20 "the device's picked control ports"
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
$work2 = Join-Path $bundles "ci-service-2"
New-AdminOnlyFolder $work2
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
Wait-Until { (Get-Content (Join-Path $env:ProgramData "dasio\instance-$second\dasio.log") -ErrorAction SilentlyContinue) -match "the device's control ports: from its config arc=4441 cmc=8801 flows=4456 process_id=1" } 20 "the second instance on its config's ports"
if ((Get-ServicePid) -ne $first) { throw "the same binary restarted the first instance: pid $first -> $(Get-ServicePid)" }
Write-Output "second instance installed beside the first (pid $first kept)"

# An install while files come and go under the data folder: a running
# device writes its state through a .tmp it renames, its log rotates
# (main CI 42801226358: the protection pass read a .tmp that had just
# gone, and the install failed).
$churnDir = Join-Path $data "instance-$instance"
$churn = Start-Job -ArgumentList $churnDir -ScriptBlock {
    param($d)
    $end = (Get-Date).AddSeconds(90)
    $i = 0
    while ((Get-Date) -lt $end) {
        $f = Join-Path $d "churn-$($i % 50).tmp"
        Set-Content -Path $f -Value "x"
        Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue
        $i++
    }
}
try {
    foreach ($k in 1..3) {
        & (Join-Path $Bundle "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock | Out-Null
    }
} finally {
    Stop-Job $churn
    Remove-Job $churn -Force
    Get-ChildItem $churnDir -Filter "churn-*.tmp" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}
if ((Get-Service "dasio-$second").Status -ne "Running") { throw "dasio-$second is not running after the installs beside the churn" }
Write-Output "three installs while files came and went in the data folder"

# A new binary for the second instance while both run: only it restarts;
# the first runs its own binary until its own install.
$newer = Join-Path $bundles "ci-newer"
New-AdminOnlyFolder $newer
Copy-Item -Force (Join-Path $Bundle "*") $newer
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
$newest = Join-Path $bundles "ci-newest"
New-AdminOnlyFolder $newest
Copy-Item -Force (Join-Path $newer "*") $newest
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
    # A held driver of an older shared-memory protocol would not find the new
    # service's memory: -KeepHeldDriver is refused then.
    $key = "HKLM:\SOFTWARE\dasio"
    $proto = (Get-ItemProperty -Path $key -Name ShmProtocol).ShmProtocol
    Set-ItemProperty -Path $key -Name ShmProtocol -Value 3
    try {
        $refused = $false
        try { & (Join-Path $newest "install.ps1") -Instance $second -Ip $ip -Config $cfg2 -Name "ci-asio-dante-2" -TestClock -KeepHeldDriver } catch { $refused = $_.Exception.Message -match "protocol 3" }
        if (-not $refused) { throw "a held driver of another protocol was kept" }
    } finally {
        Set-ItemProperty -Path $key -Name ShmProtocol -Value $proto
    }
} finally {
    $held.Close()
}
Write-Output "a held driver: refused, kept with -KeepHeldDriver, refused across a protocol change"

# An install that fails after it stopped the instance (dasio#28 A37/E20):
# the instance runs again as it was (its binary, config and service
# command), and the failure says so. The installed config held open for
# reading makes the config copy fail, past the stop and the new binary.
$ownExe = (Get-CimInstance Win32_Service -Filter "Name='$service'").PathName
$cmdBefore = $ownExe
$ownExe = [regex]::Match($ownExe, '^"([^"]+)"').Groups[1].Value
$exeBefore = (Get-FileHash $ownExe).Hash
$cfgInstalled = Join-Path $env:ProgramData "dasio\instance-$instance\device.toml"
$cfgBefore = (Get-FileHash $cfgInstalled).Hash
$lock = [System.IO.File]::Open($cfgInstalled, "Open", "Read", "Read")
try {
    $why = $null
    try { & (Join-Path $Bundle "install.ps1") -Instance $instance -Ip $ip -Config $cfg -Name "ci-asio-dante" -TestClock } catch { $why = $_.Exception.Message }
} finally {
    $lock.Close()
}
if (-not $why) { throw "the install did not fail with its config held" }
if ($why -notmatch "rolled back") { throw "the failed install does not say it rolled back: $why" }
if ((Get-Service -Name $service).Status -ne "Running") { throw "$service is $((Get-Service -Name $service).Status) after a failed install" }
if ((Get-CimInstance Win32_Service -Filter "Name='$service'").PathName -ne $cmdBefore) { throw "$service's command changed in a failed install" }
if ((Get-FileHash $ownExe).Hash -ne $exeBefore) { throw "the instance's binary is not the one it ran before the failed install" }
if ((Get-FileHash $cfgInstalled).Hash -ne $cfgBefore) { throw "the instance's config changed in a failed install" }
Write-Output "a failed install: the instance runs again as it was ($why)"

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
