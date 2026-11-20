# dasio: remove one instance (plan Task 9). Run as administrator:
#   .\uninstall.ps1 -Instance 1 [-Ip <its address>]
# Stops and removes its service and its own binary (and a logon task an
# older install made) and its expected-device file for dtray; with the last
# instance gone, dtray no longer starts at logon and the process keeper
# (task dasio-pc-keep, dasio-keep.exe, dasio#185) is removed,
# unregisters its driver entry, removes the extra address an older install.ps1
# added (installs now add none). The other
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
$dtrayCli = Join-Path $env:ProgramFiles "dasio\dtray-cli.exe"
if (-not (Get-Service -Name "dasio-*" -ErrorAction SilentlyContinue) -and (Test-Path $dtrayCli)) {
    & (Join-Path $PSScriptRoot "dtray-autostart.ps1") -Off -Exe (Join-Path $env:ProgramFiles "dasio\dtray.exe")
}
# The last instance gone: no process keeper either (dasio#185): off (its
# 5-min trigger must not start it again), stopped, any keeper left ended,
# the task removed, then its exe (a few tries: a scanner can hold a
# just-ended exe a moment), also on a rerun after a failed removal.
if (-not (Get-Service -Name "dasio-*" -ErrorAction SilentlyContinue)) {
    if (Get-ScheduledTask -TaskName "dasio-pc-keep" -ErrorAction SilentlyContinue) {
        Disable-ScheduledTask -TaskName "dasio-pc-keep" | Out-Null
    }
    # The keeper ends by itself within 2 s of its stop file; killed only
    # when it does not.
    $keepStop = Join-Path $env:ProgramData "dasio\pc-keep.stop"
    Set-Content -Path $keepStop -Value "uninstall"
    $deadline = (Get-Date).AddSeconds(15)
    while (Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue) {
        if ((Get-Date) -gt $deadline) {
            Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Seconds 2
            if (Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue) {
                throw "the process keeper did not end within 15 s; its task stays disabled: run the uninstall again"
            }
            break
        }
        Start-Sleep -Milliseconds 250
    }
    if (Get-ScheduledTask -TaskName "dasio-pc-keep" -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName "dasio-pc-keep" -Confirm:$false
    }
    $keepExe = Join-Path $env:ProgramFiles "dasio\dasio-keep.exe"
    for ($try = 1; Test-Path $keepExe; $try++) {
        try { Remove-Item $keepExe -ErrorAction Stop }
        catch { if ($try -ge 10) { throw "removing dasio-keep.exe failed 10 times: $($_.Exception.Message)" }; Start-Sleep -Milliseconds 500 }
    }
    Remove-Item $keepStop
}
Write-Output "dasio instance $Instance removed"
