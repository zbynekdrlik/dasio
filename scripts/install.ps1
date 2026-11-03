# dasio: install or update one instance (plan Task 9). Run as administrator
# from the unpacked bundle:
#   .\install.ps1 -Instance 1 -Ip <the PC's address> -Config <device.toml>
# - copies dasio.exe and dasio_driver.dll to C:\Program Files\dasio, and
#   dasio.exe again as dasio-<device name>.exe, the instance's own binary
#   (its service runs it, so Process Lasso can pin each instance by name);
#   a running instance is stopped first (its service, or the logon task an
#   older install made, which is removed); unchanged files stay, and other
#   instances keep running their own binaries,
# - refuses, before anything changes, a new dasio_driver.dll while a DAW
#   holds the installed one (a DAW keeps the driver loaded until it quits),
#   unless -KeepHeldDriver leaves that one in place,
# - moves an instance of the product's former name over: its service
#   "dante-asio-<n>" is stopped and removed, its state files are copied from
#   C:\ProgramData\dante-asio\instance-<n>, and the former files go when
#   nothing holds them,
# - adds no address: -Ip is one the PC already has. Several instances share it
#   (dstack's per-PC manager), each on its own ports and process id from its
#   config (arc_port, cmc_port, flows_port, process_id); a config that clashes
#   with another installed instance's is refused before anything changes,
# - registers the driver for the instance (HKLM: its COM class and its ASIO entry),
# - puts the device config at C:\ProgramData\dasio\instance-<n>\device.toml,
#   and moves aside a state file another config left at its state path,
# - copies dasio-tray.exe (the tray icon: each instance's state, read-only)
#   and starts it at every user's logon (HKLM Run "dasio-tray"); a running
#   tray keeps its binary until the next logon (the old file is moved aside),
# - makes the service "dasio-<n>" (LocalSystem, started at boot before any
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
    [switch]$TestClock,
    # Leave an installed driver a DAW holds as it is (the new one comes with
    # the next install after the DAWs quit); without it such an install is
    # refused.
    [switch]$KeepHeldDriver,
    # Block periods the DAW gets beyond the first (0 or 1): a heavy set's
    # late blocks still play, its outputs that many blocks later.
    [ValidateRange(0, 1)][int]$SpareBlocks = 0
)
$ErrorActionPreference = "Stop"

$bin = Join-Path $env:ProgramFiles "dasio"
$data = Join-Path $env:ProgramData "dasio\instance-$Instance"
$service = "dasio-$Instance"
$task = "dasio-$Instance"
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
$root = Join-Path $env:ProgramData "dasio"
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

# The config's device name (Dante's rule: letters, digits and '-') names
# the instance's own binary.
function Get-TomlString([string]$Path, [string]$Key) {
    foreach ($line in Get-Content $Path) {
        if ($line -match "^\s*$Key\s*=\s*([`"'])(.*)\1\s*$") { return $Matches[2] }
    }
    $null
}
$deviceName = Get-TomlString $Config "name"
if ($deviceName -and $deviceName -match '^dasio-[A-Za-z0-9-]+$') {
    # Already named after the product (iem-snv's dasio-test-1).
    $instExeName = "$deviceName.exe"
} elseif ($deviceName -and $deviceName -match '^[A-Za-z0-9-]+$') {
    $instExeName = "dasio-$deviceName.exe"
} else {
    $instExeName = "dasio-$Instance.exe"
}

# A DAW holds the installed driver while it runs: a new one cannot replace
# it. Found before anything stops.
$dll = Join-Path $bin "dasio_driver.dll"
$newDll = -not (Test-Path $dll) -or (Get-FileHash $dll).Hash -ne (Get-FileHash (Join-Path $here "dasio_driver.dll")).Hash
$dllHeld = $false
if ($newDll -and (Test-Path $dll)) {
    try { [System.IO.File]::Open($dll, "Open", "ReadWrite", "None").Close() } catch { $dllHeld = $true }
}
if ($dllHeld -and -not $KeepHeldDriver) {
    throw "a DAW holds $dll (quit it, or pass -KeepHeldDriver to keep that driver for now); nothing changed"
}

# The product's former name: that instance's service goes, its state
# comes over.
$formerService = "dante-asio-$Instance"
$formerData = Join-Path $env:ProgramData "dante-asio\instance-$Instance"
if (Get-Service -Name $formerService -ErrorAction SilentlyContinue) {
    Stop-Service -Name $formerService
    (Get-Service -Name $formerService).WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    & sc.exe delete $formerService | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc delete $formerService failed ($LASTEXITCODE)" }
    Write-Output "removed the former service $formerService"
}
if (Test-Path $formerData) {
    New-Item -ItemType Directory -Force $data | Out-Null
    Get-ChildItem $formerData -File -Filter "*.toml*" | ForEach-Object {
        $to = Join-Path $data $_.Name
        if (-not (Test-Path $to)) {
            Copy-Item $_.FullName $to
            Write-Output "copied $($_.Name) from $formerData"
        }
    }
}

$installed = Join-Path $env:ProgramFiles "dasio\dasio.exe"
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
# The files all instances share. An unchanged file stays. Every instance
# runs its own copy of dasio.exe; one an older install made still runs the
# shared one, so a new dasio.exe stops that instance for the copy and
# starts it again after.
$changed = @("dasio.exe", "dasio_driver.dll") | Where-Object {
    $there = Join-Path $bin $_
    -not (Test-Path $there) -or (Get-FileHash $there).Hash -ne (Get-FileHash (Join-Path $here $_)).Hash
}
if ($dllHeld) {
    $changed = @($changed | Where-Object { $_ -ne "dasio_driver.dll" })
    Write-Warning "kept the installed driver: a DAW holds it"
}
$paused = @()
if ($changed -contains "dasio.exe") {
    $shared = [regex]::Escape((Join-Path $bin "dasio.exe"))
    $paused = @(Get-CimInstance Win32_Service -Filter "Name LIKE 'dasio-%'" | Where-Object {
            $_.Name -ne $service -and $_.State -eq "Running" -and $_.PathName -match "$shared`""
        } | ForEach-Object { Get-Service -Name $_.Name })
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
# The tray: a running one holds its binary, which Windows lets move but not
# overwrite; the moved-aside copies go once nothing runs them.
$tray = Join-Path $bin "dasio-tray.exe"
Get-ChildItem $bin -Filter "dasio-tray.exe.old-*" -ErrorAction SilentlyContinue | ForEach-Object {
    Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue
}
if (-not (Test-Path $tray) -or (Get-FileHash $tray).Hash -ne (Get-FileHash (Join-Path $here "dasio-tray.exe")).Hash) {
    if (Test-Path $tray) { Move-Item $tray "$tray.old-$(Get-Date -Format yyyyMMddHHmmss)" -Force }
    Copy-Item (Join-Path $here "dasio-tray.exe") $tray -Force
    Write-Output "installed the tray (it starts at the next logon)"
}
Set-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -Name "dasio-tray" -Value "`"$tray`""
# The instance's own binary (its service is stopped).
$instExe = Join-Path $bin $instExeName
if (-not (Test-Path $instExe) -or (Get-FileHash $instExe).Hash -ne (Get-FileHash (Join-Path $bin "dasio.exe")).Hash) {
    Copy-Item (Join-Path $bin "dasio.exe") $instExe -Force
}
$cfg = Join-Path $data "device.toml"
Copy-Item $Config $cfg -Force

# The config's state file is this device's memory (its names and its
# subscriptions). One another config left there is another device's: it is
# moved aside, dated, so this device does not start under that name with
# those subscriptions. Kept: a state its own config made (a name set in
# Dante Controller stays).
$configName = Get-TomlString $cfg "name"
$stateFile = Get-TomlString $cfg "state_file"
if ($stateFile -and (Test-Path $stateFile)) {
    $madeBy = Get-TomlString $stateFile "config_name"
    if (-not $madeBy) { $madeBy = Get-TomlString $stateFile "name" }
    if ($madeBy -ne $configName) {
        $aside = "$stateFile.replaced-$(Get-Date -Format yyyyMMdd-HHmmss)"
        Move-Item $stateFile $aside
        Write-Output "moved aside the state of '$madeBy' (not this config's '$configName'): $aside"
    }
}

$exe = Join-Path $bin "dasio.exe"
$named = @()
if ($Name) { $named = @("--name", $Name) }
& $exe register --instance $Instance --dll (Join-Path $bin "dasio_driver.dll") @named
if ($LASTEXITCODE -ne 0) { throw "register failed ($LASTEXITCODE)" }

$log = Join-Path $data "dasio.log"
$bin_path = "`"$instExe`" service --instance $Instance --config `"$cfg`" --ip $Ip --log `"$log`""
if ($TestClock) { $bin_path += " --clock system" }
if ($SpareBlocks -gt 0) { $bin_path += " --spare-blocks $SpareBlocks" }
$existing = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($existing) {
    # Through CIM: the path's quotes reach the SCM unchanged (sc.exe's
    # argument parsing would split them).
    $r = $existing | Invoke-CimMethod -MethodName Change -Arguments @{ PathName = $bin_path; StartMode = "Automatic" }
    if ($r.ReturnValue -ne 0) { throw "changing $service failed ($($r.ReturnValue))" }
} else {
    New-Service -Name $service -BinaryPathName $bin_path -DisplayName "dasio instance $Instance" -StartupType Automatic | Out-Null
}
& sc.exe failure $service reset= 86400 actions= restart/2000/restart/5000/restart/30000 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failure $service failed ($LASTEXITCODE)" }
# Restart after an error exit too, not only after a crash.
& sc.exe failureflag $service 1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failureflag $service failed ($LASTEXITCODE)" }
Start-Service -Name $service
(Get-Service -Name $service).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))

# The former product's files, once no service runs them and no DAW holds
# its driver.
$formerBin = Join-Path $env:ProgramFiles "dante-asio"
if ((Test-Path $formerBin) -and -not (Get-Service -Name "dante-asio-*" -ErrorAction SilentlyContinue)) {
    try {
        Remove-Item -Recurse -Force $formerBin
        Write-Output "removed $formerBin"
    } catch {
        Write-Warning "kept $formerBin for now: $($_.Exception.Message)"
    }
}
Write-Output "dasio instance $Instance installed and started ($instExeName)"
