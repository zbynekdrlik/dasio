# dasio NDIS send pacer: remove the filter (all bindings) and its package.
# Run as administrator. Leaves test signing as it is.
$ErrorActionPreference = "Stop"
$component = "dasio_pacer"
if (Get-NetAdapterBinding -ComponentID $component -AllBindings -ErrorAction SilentlyContinue) {
    & netcfg.exe -v -u $component
    if ($LASTEXITCODE -ne 0) { throw "netcfg uninstall failed ($LASTEXITCODE)" }
}
pnputil /enum-drivers | Out-String -Stream | Select-String -Context 1,0 'dasiopacer.inf' | ForEach-Object {
    if ($_.Context.PreContext[0] -match '(oem\d+\.inf)') {
        pnputil /delete-driver $Matches[1] /uninstall | Out-Null
        Write-Output "removed driver package $($Matches[1])"
    }
}
Write-Output "dasio pacer removed"
