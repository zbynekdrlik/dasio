# dasio NDIS send pacer: install the filter and bind it to the Dante NIC only.
# Run as administrator from the unpacked package (dasiopacer.sys/.inf/.cat
# and the WDK test certificate .cer):
#   .\install.ps1 -Adapter <the Dante NIC's name, e.g. Ethernet>
# Needs test signing on (bcdedit /set testsigning on, then a reboot); the
# script checks it and changes nothing when it is off.
#   -RxdCpu <n>: deferred receive on, its DPC on CPU n, kept across reboots
#   (the service's Parameters: RxdEnable, RxdCpu); -RxdCpu -1 turns it off.
#   -TxMaxOut <n>: media TX's bound on frames out at the miniport, kept
#   across reboots (Parameters TxMaxOut; 0 = every slot). The default 200
#   is the FOH PC's measured value (dasio#1): with every slot (4096) out,
#   Realtek 10.79 failed frames with NDIS_STATUS_RESOURCES once ~448 were
#   out; at 200 it failed none.
param(
    [Parameter(Mandatory)][string]$Adapter,
    [ValidateRange(-2, 1023)][int]$RxdCpu = -2,
    [ValidateRange(0, 4096)][int]$TxMaxOut = 200
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

# The service's Parameters, read when the driver loads. New-Item -Force
# would replace an existing key and drop its values.
$params = "HKLM:\SYSTEM\CurrentControlSet\Services\dasiopacer\Parameters"
if (-not (Test-Path $params)) { New-Item -Path $params | Out-Null }

# Deferred receive: kept in the service's Parameters (read when the driver
# loads) and applied now.
if ($RxdCpu -ne -2) {
    $on = if ($RxdCpu -ge 0) { 1 } else { 0 }
    New-ItemProperty -Path $params -Name RxdEnable -Value $on -PropertyType DWord -Force | Out-Null
    New-ItemProperty -Path $params -Name RxdCpu -Value ([int32]$RxdCpu) -PropertyType DWord -Force | Out-Null
    & (Join-Path $here "rxd.ps1") $(if ($on) { "on" } else { "off" }) -Cpu $RxdCpu
}

# Media TX's bound: kept in the service's Parameters (read when the driver
# loads, which was before this write) and applied now.
New-ItemProperty -Path $params -Name TxMaxOut -Value ([int32]$TxMaxOut) -PropertyType DWord -Force | Out-Null
& (Join-Path $here "ktx.ps1") txset -MaxOut $TxMaxOut
