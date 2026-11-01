# dante-asio: install or update one instance (plan Task 9). Run as administrator
# from the unpacked bundle:
#   .\install.ps1 -Instance 1 -Ip <the PC's address> -Config <device.toml>
# - copies dante-asio.exe and dante_asio_driver.dll to C:\Program Files\dante-asio
#   (a running instance is stopped first: its service, or the logon task an
#   older install made, which is removed); unchanged files stay, and a new
#   dante-asio.exe stops the other running instances for the copy and starts
#   them again,
# - adds no address: -Ip is one the PC already has. Several instances share it
#   (dstack's per-PC manager), each on its own ports and process id from its
#   config (arc_port, cmc_port, flows_port, process_id); a config that clashes
#   with another installed instance's is refused before anything changes,
# - registers the driver for the instance (HKLM: its COM class and its ASIO entry),
# - puts the device config at C:\ProgramData\dante-asio\instance-<n>\device.toml,
# - makes the service "dante-asio-<n>" (LocalSystem, started at boot before any
#   logon; the SCM restarts it after 2 s, 5 s, then 30 s, also after an error
#   exit, the count reset after a day) and starts it.
param(
    [Parameter(Mandatory)][ValidateRange(1, 16)][int]$Instance,
    [Parameter(Mandatory)][string]$Ip,
    [Parameter(Mandatory)][string]$Config,
    # The name DAWs list the driver under (HKLM\SOFTWARE\ASIO), e.g. master-asio-dante.
    [string]$Name = "",
    # CI only: the stack's test clock (this machine's clock, no dantesync).
    # Never on a real Dante network.
    [switch]$TestClock
)
$ErrorActionPreference = "Stop"

$bin = Join-Path $env:ProgramFiles "dante-asio"
$data = Join-Path $env:ProgramData "dante-asio\instance-$Instance"
$service = "dante-asio-$Instance"
$task = "dante-asio-$Instance"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# The bundle's checksums first.
Get-Content (Join-Path $here "SHA256SUMS") | ForEach-Object {
    # Not $name: PowerShell names are case-blind, so it would overwrite -Name.
    $hash, $file = $_ -split "  ", 2
    $got = (Get-FileHash (Join-Path $here $file) -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $hash) { throw "$file does not match SHA256SUMS" }
}

# The address is the PC's own: no extra address any more.
if (-not (Get-NetIPAddress -IPAddress $Ip -AddressFamily IPv4 -ErrorAction SilentlyContinue)) {
    throw "$Ip is not one of this PC's addresses; nothing changed"
}

# The ports and process id each device takes on the shared address (the
# stack's defaults when the config leaves them out).
function Get-DeviceSlots([string]$Path) {
    $slots = @{ arc_port = 4440; cmc_port = 8800; flows_port = 4455; process_id = 0 }
    foreach ($line in Get-Content $Path) {
        if ($line -match '^\s*(arc_port|cmc_port|flows_port|process_id)\s*=\s*(\d+)') {
            $slots[$Matches[1]] = [int]$Matches[2]
        }
    }
    $slots
}
$mine = Get-DeviceSlots $Config
$root = Join-Path $env:ProgramData "dante-asio"
Get-ChildItem $root -Directory -Filter "instance-*" -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne "instance-$Instance" } | ForEach-Object {
    $other = Join-Path $_.FullName "device.toml"
    if (Test-Path $other) {
        $theirs = Get-DeviceSlots $other
        $ports = @($theirs.arc_port, $theirs.cmc_port, $theirs.flows_port)
        foreach ($key in "arc_port", "cmc_port", "flows_port") {
            if ($ports -contains $mine[$key]) { throw "$key $($mine[$key]) is taken by $($_.Name); nothing changed" }
        }
        if ($mine.process_id -eq $theirs.process_id) { throw "process_id $($mine.process_id) is taken by $($_.Name); nothing changed" }
    }
}

$installed = Join-Path $env:ProgramFiles "dante-asio\dante-asio.exe"
if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
    # The SCM's Stop: the device ends within its stop's 7 s.
    Stop-Service -Name $service
    (Get-Service -Name $service).WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
}
if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) {
    # An older install's logon task: asked to stop (never killed), then removed.
    if (Test-Path $installed) {
        & $installed stop --instance $Instance
        if ($LASTEXITCODE -ne 0) { throw "stop failed ($LASTEXITCODE)" }
    }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-ScheduledTask -TaskName $task).State -eq "Running") {
        if ((Get-Date) -gt $deadline) { throw "instance $Instance did not stop within 15 s; nothing changed" }
        Start-Sleep -Milliseconds 200
    }
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
}

New-Item -ItemType Directory -Force $bin, $data | Out-Null
# The files all instances share. An unchanged file stays (a running
# instance holds dante-asio.exe open). A new dante-asio.exe stops every
# other running instance for the copy and starts it again after.
$changed = @("dante-asio.exe", "dante_asio_driver.dll") | Where-Object {
    $there = Join-Path $bin $_
    -not (Test-Path $there) -or (Get-FileHash $there).Hash -ne (Get-FileHash (Join-Path $here $_)).Hash
}
$paused = @()
if ($changed -contains "dante-asio.exe") {
    $paused = @(Get-Service -Name "dante-asio-*" | Where-Object { $_.Name -ne $service -and $_.Status -eq "Running" })
    foreach ($s in $paused) {
        Write-Output "stopping $($s.Name) for the new binary"
        Stop-Service -Name $s.Name
        $s.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    }
}
try {
    foreach ($file in $changed) { Copy-Item (Join-Path $here $file) $bin -Force }
} finally {
    foreach ($s in $paused) {
        Start-Service -Name $s.Name
        (Get-Service -Name $s.Name).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))
        Write-Output "$($s.Name) runs the new binary"
    }
}
$cfg = Join-Path $data "device.toml"
Copy-Item $Config $cfg -Force

$exe = Join-Path $bin "dante-asio.exe"
$named = @()
if ($Name) { $named = @("--name", $Name) }
& $exe register --instance $Instance --dll (Join-Path $bin "dante_asio_driver.dll") @named
if ($LASTEXITCODE -ne 0) { throw "register failed ($LASTEXITCODE)" }

$log = Join-Path $data "dante-asio.log"
$bin_path = "`"$exe`" service --instance $Instance --config `"$cfg`" --ip $Ip --log `"$log`""
if ($TestClock) { $bin_path += " --clock system" }
$existing = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($existing) {
    # Through CIM: the path's quotes reach the SCM unchanged (sc.exe's
    # argument parsing would split them).
    $r = $existing | Invoke-CimMethod -MethodName Change -Arguments @{ PathName = $bin_path; StartMode = "Automatic" }
    if ($r.ReturnValue -ne 0) { throw "changing $service failed ($($r.ReturnValue))" }
} else {
    New-Service -Name $service -BinaryPathName $bin_path -DisplayName "dante-asio instance $Instance" -StartupType Automatic | Out-Null
}
& sc.exe failure $service reset= 86400 actions= restart/2000/restart/5000/restart/30000 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failure $service failed ($LASTEXITCODE)" }
# Restart after an error exit too, not only after a crash.
& sc.exe failureflag $service 1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failureflag $service failed ($LASTEXITCODE)" }
Start-Service -Name $service
(Get-Service -Name $service).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))
Write-Output "dante-asio instance $Instance installed and started"
