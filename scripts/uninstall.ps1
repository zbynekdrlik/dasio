# dasio: remove one instance (plan Task 9). Run as administrator:
#   .\uninstall.ps1 -Instance 1 [-Ip <its address>]
# Stops and removes its service and its own binary (and a logon task an
# older install made) and its expected-device file for dtray; with the last
# instance gone, dtray no longer starts at logon,
# unregisters its driver entry, removes the extra address an older install.ps1
# added (installs now add none). The
# files in C:\Program Files\dasio stay (other instances use them); its
# data folder stays (the device's names).
param(
    [Parameter(Mandatory)][ValidateRange(1, 16)][int]$Instance,
    # Only for the extra address an older install added; an instance
    # installed with -Iface (or on the PC's own address) needs none.
    [string]$Ip = "",
    [string]$Name = ""
)
$ErrorActionPreference = "Stop"

$task = "dasio-$Instance"
$exe = Join-Path $env:ProgramFiles "dasio\dasio.exe"
$service = "dasio-$Instance"
# The instance's own binary (dasio-<device name>.exe), removed with its service.
$own = $null
$svc = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($svc -and $svc.PathName -match '^"([^"]+\\dasio-[^"\\]+\.exe)"') { $own = $Matches[1] }
if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
    Stop-Service -Name $service
    (Get-Service -Name $service).WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
    & sc.exe delete $service | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc delete $service failed ($LASTEXITCODE)" }
}
if ($own -and (Test-Path $own)) {
    Remove-Item -Force $own
}
if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) {
    if (Test-Path $exe) {
        # Asked to stop (never killed).
        & $exe stop --instance $Instance
        if ($LASTEXITCODE -ne 0) { throw "stop failed ($LASTEXITCODE)" }
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-ScheduledTask -TaskName $task).State -eq "Running") {
            if ((Get-Date) -gt $deadline) { throw "instance $Instance did not stop within 15 s" }
            Start-Sleep -Milliseconds 200
        }
    }
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
}
if (Test-Path $exe) {
    $named = @()
    if ($Name) { $named = @("--name", $Name) }
    & $exe unregister --instance $Instance @named
    if ($LASTEXITCODE -ne 0) { throw "unregister failed ($LASTEXITCODE)" }
}
# Only an extra address an older install.ps1 added (an address the interface
# had before stays: it is the PC's own).
$added = Join-Path $env:ProgramData "dasio\instance-$Instance\added-ip"
if ($Ip -and (Test-Path $added) -and ((Get-Content $added) -eq $Ip)) {
    if (Get-NetIPAddress -IPAddress $Ip -ErrorAction SilentlyContinue) {
        Remove-NetIPAddress -IPAddress $Ip -Confirm:$false
    }
    Remove-Item $added
}
# The instance's expected-device file (dtray would show it as not running).
$named = Join-Path $env:ProgramData "dasio\instance-$Instance\dtray-device"
if (Test-Path $named) {
    $dev = (Get-Content $named -Raw).Trim()
    $file = Join-Path $env:ProgramData "dstack\devices\$dev.device"
    if ($dev -and (Test-Path $file)) { Remove-Item $file }
    Remove-Item $named
}
# The last instance gone: dtray no longer starts at logon.
$dtray = Join-Path $env:ProgramFiles "dasio\dtray.exe"
if (-not (Get-Service -Name "dasio-*" -ErrorAction SilentlyContinue) -and (Test-Path $dtray)) {
    # dtray is a GUI program: `&` would not wait for it nor set its exit code.
    $set = Start-Process $dtray -ArgumentList "--autostart", "off" -Wait -PassThru -WindowStyle Hidden
    if ($set.ExitCode -ne 0) { throw "dtray --autostart off failed ($($set.ExitCode))" }
}
Write-Output "dasio instance $Instance removed"
