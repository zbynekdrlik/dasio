# dasio: dtray (dstack's tray) on or off at logon, for install.ps1 and
# uninstall.ps1 (dasio#211).
#   .\dtray-autostart.ps1 -On  -Exe "C:\Program Files\dasio\dtray.exe"
#   .\dtray-autostart.ps1 -Off -Exe "C:\Program Files\dasio\dtray.exe"
# - always runs `dtray-cli --autostart on|off` (next to -Exe): dtray's own
#   Run value in the current user's hive;
# - run as SYSTEM (the PCs install from a SYSTEM scheduled task), the current
#   user's hive is SYSTEM's own, which no one logs on with: the same value,
#   `dtray` = the quoted path of -Exe (dtray's `run_value`), is also set in,
#   or removed from, every loaded user hive (HKEY_USERS\S-1-5-21-...). An
#   existing Run key is never made again (that would drop its other values).
#   No HKLM Run value: a user who has the per-user one would start two trays.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [switch]$On,
    [switch]$Off
)
$ErrorActionPreference = "Stop"
if ($On -eq $Off) { throw "exactly one of -On and -Off" }
$cli = Join-Path (Split-Path -Parent $Exe) "dtray-cli.exe"
& $cli --autostart $(if ($On) { "on" } else { "off" })
if ($LASTEXITCODE -ne 0) { throw "dtray-cli --autostart failed ($LASTEXITCODE)" }
if (-not [System.Security.Principal.WindowsIdentity]::GetCurrent().IsSystem) { return }
$value = '"' + $Exe + '"'
$sids = @(Get-ChildItem Registry::HKEY_USERS |
    Where-Object { $_.PSChildName -match '^S-1-5-21-[0-9-]+$' } |
    ForEach-Object { $_.PSChildName })
foreach ($sid in $sids) {
    $key = "Registry::HKEY_USERS\$sid\Software\Microsoft\Windows\CurrentVersion\Run"
    if ($On) {
        if (-not (Test-Path $key)) { New-Item -Path $key | Out-Null }
        Set-ItemProperty -Path $key -Name "dtray" -Value $value
        Write-Output "dtray starts at the logon of $sid"
    } elseif (Get-ItemProperty $key -Name "dtray" -ErrorAction SilentlyContinue) {
        Remove-ItemProperty -Path $key -Name "dtray"
        Write-Output "dtray no longer starts at the logon of $sid"
    }
}
if ($sids.Count -eq 0) {
    Write-Output "no user hive is loaded: a user's dtray autostart waits for an install while that user is logged on"
}
