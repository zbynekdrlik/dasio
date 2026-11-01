# dasio NDIS send pacer: install the filter and bind it to the Dante NIC only.
# Run as administrator from the unpacked package (dasiopacer.sys/.inf/.cat
# and the WDK test certificate .cer):
#   .\install.ps1 -Adapter <the Dante NIC's name, e.g. Ethernet>
# Needs test signing on (bcdedit /set testsigning on, then a reboot); the
# script checks it and changes nothing when it is off.
param(
    [Parameter(Mandatory)][string]$Adapter
)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$component = "dasio_pacer"

if (-not ((bcdedit /enum "{current}") -match "testsigning\s+Yes")) {
    throw "test signing is off on this PC (bcdedit /set testsigning on, then reboot); nothing changed"
}
$nic = Get-NetAdapter -Name $Adapter
foreach ($f in "dasiopacer.sys", "dasiopacer.inf", "dasiopacer.cat") {
    if (-not (Test-Path (Join-Path $here $f))) { throw "the package lacks $f; nothing changed" }
}

# The WDK's test certificate signs the package: trust it on this PC.
Get-ChildItem $here -Filter *.cer | ForEach-Object {
    Import-Certificate -FilePath $_.FullName -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    Import-Certificate -FilePath $_.FullName -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
    Write-Output "trusted test certificate $($_.Name)"
}

& netcfg.exe -v -l (Join-Path $here "dasiopacer.inf") -c s -i $component
if ($LASTEXITCODE -ne 0) { throw "netcfg install failed ($LASTEXITCODE)" }

# Bound to the Dante NIC only.
Get-NetAdapterBinding -ComponentID $component -AllBindings | ForEach-Object {
    if ($_.Name -eq $nic.Name) {
        Enable-NetAdapterBinding -Name $_.Name -ComponentID $component
    } else {
        Disable-NetAdapterBinding -Name $_.Name -ComponentID $component
    }
}
Get-NetAdapterBinding -ComponentID $component -AllBindings | Select-Object Name, Enabled | Format-Table -AutoSize | Out-String | Write-Output
$svc = Get-Service dasiopacer
Write-Output "dasiopacer service: $($svc.Status); bound to $($nic.Name)"
