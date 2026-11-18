# dasio: install or update one instance (plan Task 9). Run as administrator
# from the unpacked bundle:
#   .\install.ps1 -Instance 1 -Ip <the PC's address> -Config <device.toml>
#   .\install.ps1 -Instance 1 -Iface <its interface name> -Config <device.toml>
# - refuses, before anything changes, a bundle folder, bundle file or config
#   anyone but SYSTEM and the administrators could change (or swap through a
#   folder above it): keep both in an admin-only folder such as
#   C:\ProgramData\dasio\bundles\<name> (dasio#28 E19),
# - makes C:\ProgramData\dasio admin-only (users read it); refuses, before
#   changing it, a link in it or above it, and an item in it someone else
#   owns or has an entry of its own for,
# - applies dasio's baseline PC setup with the PC's own profile
#   (`dasio pc-profile apply`, dasio#128) before the instance is touched;
#   refuses, naming the step, a profile without the device adapter's
#   nic_guid and a must-have item left unset (exit 2: the Dante NIC's
#   settings, test signing, a profile that does not load),
# - with -NicDriver <folder> only (never by default: a NIC driver change
#   restarts the adapter, so it runs with someone on site), installs the
#   Dante NIC family's pinned driver package before that step: the folder
#   (inside this bundle folder) must be exactly the baseline's package,
#   every file's SHA256 and the catalog's signer checked by `dasio pc-profile
#   nic-driver` (dasio#128); refuses on any difference with nothing changed,
#   and refuses when the adapter does not run the pinned version afterwards;
#   pnputil gives the driver to every device its INF matches (a second
#   Realtek NIC restarts too), and a running instance loses its NIC for
#   those seconds (the step comes before the instance's stop),
# - copies dasio.exe and dasio_driver.dll to C:\Program Files\dasio, and
#   dasio.exe again as dasio-<device name>.exe, the instance's own binary
#   (its service runs it, so Process Lasso can pin each instance by name);
#   a running instance is stopped first (its service, or the logon task an
#   older install made, which is removed); unchanged files stay, and other
#   instances keep running their own binaries,
# - refuses, before anything changes, a new dasio_driver.dll while a DAW
#   holds the installed one (a DAW keeps the driver loaded until it quits),
#   unless -KeepHeldDriver leaves that one in place,
# - adds no address: -Ip is one the PC already has, or -Iface names the
#   interface (its alias, as Get-NetIPAddress lists it) whose one IPv4
#   address the device takes at each start (exactly one of the two).
#   Several instances share it (dstack's per-PC manager), each on its own
#   ports and process id from its config (arc_port, cmc_port, flows_port,
#   process_id; a config naming none takes the first free set at its start,
#   dasio#89); a config that clashes with another installed instance's is
#   refused before anything changes,
# - registers the driver for the instance (HKLM: its COM class and its ASIO entry;
#   not for a bridge instance, whose config has a [bridge] table),
# - puts the device config at C:\ProgramData\dasio\instance-<n>\device.toml,
#   and moves aside a state file another config left at its state path,
# - copies dasio.exe as dasio-keep.exe and (re)starts the SYSTEM task
#   dasio-pc-keep (`dasio pc-profile keep`, dasio#185, at boot and every
#   5 min): the [[process]] entries' affinity and priority, held by dasio
#   (what Process Lasso did);
# - copies dtray.exe and dtray-cli.exe (dstack's tray: every local dstack device, with dasio's
#   line per instance) and starts it at the installing user's logon
#   (`dtray-cli --autostart on`); removes the former dasio-tray; writes the
#   instance's expected-device file %ProgramData%\dstack\devices\<device>.device,
# - makes the service "dasio-<n>" (LocalSystem, started at boot before any
#   logon; the SCM restarts it after 2 s, 5 s, then 30 s, also after an error
#   exit, the count reset after a day) and starts it,
# - after the instance's stop, a failure brings it back as it was (its
#   binary, config and service command, kept in instance-<n>\rollback until
#   the install ends) and says so; a first install has nothing to bring back.
param(
    [Parameter(Mandatory)][ValidateRange(1, 16)][int]$Instance,
    # The device's address (one the PC has), or -Iface: exactly one.
    [string]$Ip = "",
    # The device's interface by name (its alias): the device takes the
    # interface's one IPv4 address at each start (dasio#29).
    [string]$Iface = "",
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
    [ValidateRange(0, 1)][int]$SpareBlocks = 0,
    # The dsync service as the only PTP time (`--clock dsync`). Without it
    # the device follows Dante PTP in process (needs Npcap) and falls back
    # to the service.
    [switch]$DsyncClock,
    # A folder in this bundle folder holding the Dante NIC's pinned driver
    # package (e.g. FOH's: pnputil /export-driver <oem inf> <folder>):
    # installed, after the check, before the baseline step (dasio#128).
    [string]$NicDriver = ""
)
$ErrorActionPreference = "Stop"
if ($TestClock -and $DsyncClock) { throw "-TestClock and -DsyncClock exclude each other; nothing changed" }
if ($TestClock -and $NicDriver) { throw "-TestClock and -NicDriver exclude each other (no Dante NIC under the test clock); nothing changed" }
if ([bool]$Ip -eq [bool]$Iface) { throw "give exactly one of -Ip <address> and -Iface <interface name>; nothing changed" }

$bin = Join-Path $env:ProgramFiles "dasio"
$data = Join-Path $env:ProgramData "dasio\instance-$Instance"
$service = "dasio-$Instance"
$task = "dasio-$Instance"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# Elevated, this script installs only what non-administrators cannot change
# (dasio#28 E19). A bundle in a folder users may change (any folder made
# directly under C:\ gives Authenticated Users Modify) could be swapped
# after its checksums are read; so could a config.
# Who counts as an administrator: SYSTEM, Administrators, TrustedInstaller,
# the local administrators and the user running this.
$trusted = @("S-1-5-18", "S-1-5-32-544", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
    [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
try {
    $trusted += @(Get-LocalGroupMember -SID "S-1-5-32-544" | ForEach-Object { $_.SID.Value })
} catch {
    Write-Warning "the local administrators could not be listed ($($_.Exception.Message)); only SYSTEM, Administrators, TrustedInstaller and you count"
}
# Rights that change an item: write or append data (add a file or folder),
# delete a child, delete, change permissions, take ownership, generic write
# and generic all.
$ChangeRights = 0x500D0046
# Rights on a folder above an item that let its holder swap the item:
# delete the folder or a child, change permissions, take ownership,
# generic all.
$SwapRights = 0x100D0040
# Everyone else who can change $Path: its owner (an owner can always grant
# itself more) and each allow entry with one of $Rights (with
# -ExplicitOnly, only the entries $Path has of its own, not inherited).
# CREATOR OWNER, CREATOR GROUP and OWNER RIGHTS stand for the owner.
function Get-OtherWriters([string]$Path, [int]$Rights, [switch]$ExplicitOnly) {
    # An item gone since it was listed (a running device renames its state
    # .tmp, its log rotates) has nobody who can change it.
    try {
        $acl = Get-Acl -LiteralPath $Path
    } catch {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        throw
    }
    $sids = @()
    $owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
    if ($trusted -notcontains $owner) { $sids += $owner }
    foreach ($rule in $acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        $sid = $rule.IdentityReference.Value
        if ($rule.AccessControlType -ne "Allow") { continue }
        if ($ExplicitOnly -and $rule.IsInherited) { continue }
        if ($trusted -contains $sid -or (@("S-1-3-0", "S-1-3-1", "S-1-3-4") -contains $sid)) { continue }
        # An inherit-only entry gives nothing on $Path itself, but an item's
        # own one is passed to everything in it, and protecting a folder
        # above does not remove it.
        $inheritOnly = [int]($rule.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0
        if ($inheritOnly -and -not $ExplicitOnly) { continue }
        if (([int]$rule.FileSystemRights -band $Rights) -ne 0) { $sids += $sid }
    }
    foreach ($sid in @($sids | Select-Object -Unique)) {
        $who = $sid
        try { $who = (New-Object System.Security.Principal.SecurityIdentifier $sid).Translate([System.Security.Principal.NTAccount]).Value } catch { Write-Verbose "no name for $sid" }
        "$who (on $Path)"
    }
}
# $Item and every folder above it.
function Get-Chain([System.IO.FileSystemInfo]$Item) {
    $Item
    $up = if ($Item -is [System.IO.DirectoryInfo]) { $Item.Parent } else { $Item.Directory }
    while ($up) {
        $up
        $up = $up.Parent
    }
}
function Test-Link([System.IO.FileSystemInfo]$Item) {
    [int]($Item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
}
# Refuses $Path when it or a folder above it is a link (the permissions read
# would be the target's, never those of the folders above the target), when
# anyone else can change it, or swap it through a folder above it.
function Assert-AdminOnly([string]$Path, [string]$What) {
    $chain = @(Get-Chain (Get-Item -LiteralPath $Path -Force))
    $links = @($chain | Where-Object { Test-Link $_ } | ForEach-Object { $_.FullName })
    if ($links.Count) {
        throw "$What goes through a link ($($links -join ', ')): run the install from a plain admin-only folder; nothing changed"
    }
    $found = @(Get-OtherWriters $chain[0].FullName $ChangeRights)
    foreach ($dir in @($chain | Select-Object -Skip 1)) {
        $found += @(Get-OtherWriters $dir.FullName $SwapRights)
    }
    if ($found.Count) {
        throw "$What can be changed by $($found -join ', '): put the bundle and the config in a folder only administrators may change (e.g. C:\ProgramData\dasio\bundles\<name>) and run the install from there; nothing changed"
    }
}
Assert-AdminOnly $here "the bundle folder $here"
$Config = (Resolve-Path -LiteralPath $Config).ProviderPath
Assert-AdminOnly $Config "the config $Config"

# The bundle's checksums, read where nobody else can change them.
$sumsFile = Join-Path $here "SHA256SUMS"
$writers = @(Get-OtherWriters $sumsFile $ChangeRights)
if ($writers.Count) { throw "SHA256SUMS in the bundle can be changed by $($writers -join ', '); nothing changed" }
Get-Content $sumsFile | ForEach-Object {
    # Not $name: PowerShell names are case-blind, so it would overwrite -Name.
    $hash, $file = $_ -split "  ", 2
    $bundled = Join-Path $here $file
    $writers = @(Get-OtherWriters $bundled $ChangeRights)
    if ($writers.Count) { throw "$file in the bundle can be changed by $($writers -join ', '); nothing changed" }
    $got = (Get-FileHash $bundled -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $hash) { throw "$file does not match SHA256SUMS" }
}

# The address is the PC's own: no extra address any more. An interface by
# name must have exactly one IPv4 address now (the device's rule at start).
if ($Iface) {
    # The name goes into the service's command line in quotes.
    if ($Iface.Contains('"')) { throw "-Iface: an interface name with a quote is not supported; nothing changed" }
    # The exact name, as the device compares it (-InterfaceAlias would take
    # a wildcard and ignore the case).
    $ifaceAddrs = @(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.InterfaceAlias -ceq $Iface })
    if ($ifaceAddrs.Count -ne 1) {
        throw "the interface '$Iface' has $($ifaceAddrs.Count) IPv4 addresses (or does not exist), the device needs exactly one; nothing changed"
    }
    $place = "--iface `"$Iface`""
} else {
    if (-not (Get-NetIPAddress -IPAddress $Ip -AddressFamily IPv4 -ErrorAction SilentlyContinue)) {
        throw "$Ip is not one of this PC's addresses; nothing changed"
    }
    $place = "--ip $Ip"
}

# dasio never runs without its NDIS filter on a Dante network (owner
# 15.11.2026, dasio#125): the device's adapter must have dasio_pacer bound
# (drivers/ndis-pacer/scripts/install.ps1 -Adapter <it>, test signing on).
# Only the test clock (CI) runs without it.
if (-not $TestClock) {
    # The filter is test-signed: with Secure Boot on, Windows never loads it.
    $secure = $false
    try { $secure = Confirm-SecureBootUEFI } catch { $secure = $false }
    if ($secure) {
        throw "Secure Boot is on: dasio's test-signed NDIS filter cannot load; switch Secure Boot off in the firmware setup (on site), then bcdedit /set testsigning on and reboot; nothing changed"
    }
    # The config's `iface = "<alias>"` wins over -Ip/-Iface at the start
    # (place.rs): its adapter is the one that must carry the filter.
    $configIface = (Select-String -LiteralPath $Config -Pattern '^\s*iface\s*=\s*"([^"]*)"' | Select-Object -First 1)
    $alias = if ($configIface) { $configIface.Matches[0].Groups[1].Value } elseif ($Iface) { $Iface } else { (Get-NetIPAddress -IPAddress $Ip -AddressFamily IPv4).InterfaceAlias }
    $binding = Get-NetAdapterBinding -Name $alias -ComponentID dasio_pacer -ErrorAction SilentlyContinue
    if (-not $binding -or -not $binding.Enabled) {
        throw "dasio's NDIS filter (dasio_pacer) is not bound to '$alias': install it first (bcdedit /set testsigning on + reboot, then drivers/ndis-pacer/scripts/install.ps1 -Adapter '$alias'); dasio does not run without it; nothing changed"
    }
    Write-Output "dasio_pacer is bound to '$alias': audio TX goes through dasio's NDIS filter"
}

# The ports and process id each device takes on the shared address (the
# stack's defaults when the config leaves them out), and whether the config
# names any (one naming none takes the first free slot at its start,
# dasio#89, so beside another instance its slot is not known here).
function Get-DeviceSlots([string]$Path) {
    $slots = @{ arc_port = 4440; cmc_port = 8800; flows_port = 4455; process_id = 0; named = $false }
    foreach ($line in Get-Content $Path) {
        if ($line -match '^\s*(arc_port|cmc_port|flows_port|process_id)\s*=\s*(\d+)') {
            $slots[$Matches[1]] = [int]$Matches[2]
            $slots.named = $true
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
        if (-not ($mine.named -and $theirs.named)) {
            Write-Warning "this config or $($_.Name)'s names no arc_port, cmc_port, flows_port or process_id: that device takes the first free slot at each start, which may be one the other names; name the ports in both configs when instances share this PC"
        }
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
# The shared memory's protocol between the driver and the service (shm's
# VERSION; shm's tests check the two agree). A held driver of another
# protocol would not find the new service's memory and play silence.
$ShmProtocol = 6
$protocolKey = "HKLM:\SOFTWARE\dasio"
$heldProtocol = (Get-ItemProperty -Path $protocolKey -Name ShmProtocol -ErrorAction SilentlyContinue).ShmProtocol
if ($dllHeld -and $heldProtocol -ne $ShmProtocol) {
    throw "a DAW holds $dll, which speaks shared-memory protocol $(if ($heldProtocol) { $heldProtocol } else { 'before 4' }); this service speaks ${ShmProtocol}: quit the DAW first; nothing changed"
}

# dasio's data folder (configs, states, logs, the rollback copies, the PC
# profile): only SYSTEM and administrators change what is in it, users read
# it (dasio#28 E19). ProgramData lets users add files and folders: a
# rollback folder a user made first would be theirs, and the exe the
# install keeps there goes back into Program Files on a failure, for the
# LocalSystem service to run. The folder is protected from what ProgramData
# passes down; what is in it inherits only that. Nothing is changed below
# it by hand: a link there, or an item someone else owns or may change, is
# refused for an administrator to look at (a recursive change would follow
# a link a user planted).
function Get-Tree([System.IO.DirectoryInfo]$Dir) {
    # A folder gone since it was listed has nothing in it.
    try {
        $items = $Dir.GetFileSystemInfos()
    } catch {
        if (-not (Test-Path -LiteralPath $Dir.FullName)) { return }
        throw
    }
    foreach ($item in $items) {
        $item
        $isDir = [int]($item.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0
        if ($isDir -and -not (Test-Link $item)) { Get-Tree $item }
    }
}
function Protect-DataFolder([string]$Root) {
    New-Item -ItemType Directory -Force $Root | Out-Null
    $rootItem = Get-Item -LiteralPath $Root -Force
    $chain = @(Get-Chain $rootItem)
    $links = @($chain | Where-Object { Test-Link $_ } | ForEach-Object { $_.FullName })
    if ($links.Count) { throw "$Root goes through a link ($($links -join ', ')); nothing changed" }
    $tree = @(Get-Tree $rootItem)
    $links = @($tree | Where-Object { Test-Link $_ } | ForEach-Object { $_.FullName })
    if ($links.Count) { throw "links under ${Root}: $($links -join ', ') (remove them; never what they point to); nothing changed" }
    # Before the change, what protecting the folder would not undo: an owner
    # who is not an administrator, an entry of an item's own for someone
    # else, a folder above that lets someone else swap the folder.
    $found = @(Get-OtherWriters $Root $ChangeRights -ExplicitOnly)
    foreach ($dir in @($chain | Select-Object -Skip 1)) {
        $found += @(Get-OtherWriters $dir.FullName $SwapRights)
    }
    foreach ($item in $tree) {
        $found += @(Get-OtherWriters $item.FullName $ChangeRights -ExplicitOnly)
    }
    if ($found.Count) {
        throw "in ${Root}, $($found -join ', ') can change what dasio's service runs or reads: remove those entries or items; nothing changed"
    }
    $sec = New-Object System.Security.AccessControl.DirectorySecurity
    $sec.SetAccessRuleProtection($true, $false)
    foreach ($grant in @(@("S-1-5-18", "FullControl"), @("S-1-5-32-544", "FullControl"), @("S-1-5-32-545", "ReadAndExecute"))) {
        $sid = New-Object System.Security.Principal.SecurityIdentifier $grant[0]
        $sec.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule($sid, $grant[1], "ContainerInherit, ObjectInherit", "None", "Allow")))
    }
    $rootItem.SetAccessControl($sec)
    # What is in it now inherits only that (listed again: files came and
    # went meanwhile).
    $found = @(Get-Tree $rootItem | ForEach-Object { Get-OtherWriters $_.FullName $ChangeRights })
    if ($found.Count) {
        throw "in ${Root}, $($found -join ', ') can still change what dasio's service runs or reads after $Root was made admin-only; fix those by hand"
    }
}
Protect-DataFolder $root

# The built-in baseline (dasio#128) with this PC's own profile, set before
# the instance is touched, by the bundle's dasio.exe (the bundle and the
# data folder are admin-only by now). A must-have item it cannot set (exit
# 2: the Dante NIC's settings, test signing, a profile that does not load)
# refuses the install, the instance as it was; other problems (a reboot
# pending, a NIC driver to install) are listed and the install goes on. The
# service checks it again at start and every 10 min.
$applied = 0
if (-not $TestClock) {
    # The NIC's must-have settings are checked on the profile's nic_guid:
    # without it they are not checked at all, and it must be the adapter the
    # device runs on.
    $profilePath = Join-Path $root "pc-profile.toml"
    $adapterGuid = (Get-NetAdapter | Where-Object { $_.Name -ceq $alias } | Select-Object -First 1).InterfaceGuid
    $profileGuid = if (Test-Path $profilePath) { Get-TomlString $profilePath "nic_guid" } else { $null }
    if (-not $profileGuid) {
        throw "the baseline step: $profilePath has no nic_guid, so the Dante NIC's must-have settings go unchecked: write nic_guid = `"$adapterGuid`" ('$alias') there; nothing changed"
    }
    if ($profileGuid -ne $adapterGuid) {
        throw "the baseline step: $profilePath names the Dante NIC '$profileGuid', but the device runs on '$alias' ($adapterGuid): set nic_guid = `"$adapterGuid`" there; nothing changed"
    }
    # The Dante NIC's pinned driver package (dasio#128), only when asked.
    # dasio decides whether the folder is exactly the baseline's package
    # (every file's SHA256, the catalog's signer) before pnputil touches
    # anything; the NIC's settings, which a driver change resets, are set
    # again by the baseline step right after.
    if ($NicDriver) {
        $pkgDir = (Resolve-Path -LiteralPath $NicDriver).ProviderPath.TrimEnd('\')
        $bundleDir = (Resolve-Path -LiteralPath $here).ProviderPath.TrimEnd('\') + '\'
        if (-not ($pkgDir + '\').StartsWith($bundleDir, [System.StringComparison]::OrdinalIgnoreCase) -or ($pkgDir + '\') -eq $bundleDir) {
            throw "the NIC driver step: $pkgDir is not a folder inside the bundle folder $here (an admin-only place, so nothing swaps the package between the check and the install); nothing changed"
        }
        Assert-AdminOnly $pkgDir "the NIC driver folder $pkgDir"
        $pkgFiles = @(Get-ChildItem -LiteralPath $pkgDir -File -Recurse -Force)
        foreach ($f in $pkgFiles) {
            $writers = @(Get-OtherWriters $f.FullName $ChangeRights)
            if ($writers.Count) { throw "the NIC driver step: $($f.FullName) can be changed by $($writers -join ', '); nothing changed" }
        }
        $cats = @($pkgFiles | Where-Object { $_.Extension -ieq ".cat" })
        if ($cats.Count -ne 1) {
            throw "the NIC driver step: $pkgDir holds $($cats.Count) catalog (.cat) files, a driver package has one; nothing changed"
        }
        $sig = Get-AuthenticodeSignature -LiteralPath $cats[0].FullName
        $nicAdapter = Get-NetAdapter | Where-Object { $_.InterfaceGuid -eq $adapterGuid } | Select-Object -First 1
        $hwids = @((Get-PnpDeviceProperty -InstanceId $nicAdapter.PnPDeviceID -KeyName DEVPKEY_Device_HardwareIds).Data)
        $verifyArgs = @("pc-profile", "nic-driver")
        foreach ($id in $hwids) { $verifyArgs += @("--hardware-id", $id) }
        foreach ($f in $pkgFiles) {
            # A file in a subfolder keeps its folder: never a file of the package.
            $rel = $f.FullName.Substring($pkgDir.Length + 1)
            $verifyArgs += @("--file", "$rel=$((Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash)")
        }
        # An unsigned catalog has no certificate: "none" (an empty argument
        # never reaches dasio on Windows PowerShell), refused as not signed.
        $signer = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "none" }
        $verifyArgs += @("--signature", "$($sig.Status)", "--signer", $signer)
        $verdict = @(& (Join-Path $here "dasio.exe") @verifyArgs)
        $verified = $LASTEXITCODE
        $verdict | ForEach-Object { Write-Output $_ }
        if ($verified -ne 0) {
            throw "the NIC driver step: dasio refused the package in $pkgDir (the line above); nothing changed"
        }
        $inf = ($verdict | Where-Object { $_ -like "INF *" } | Select-Object -First 1).Substring(4)
        $wantVersion = ($verdict | Where-Object { $_ -like "VERSION *" } | Select-Object -First 1).Substring(8)
        Write-Output "installing $inf ($wantVersion) for '$alias': the adapter restarts, with every other device the INF matches"
        & pnputil.exe /add-driver (Join-Path $pkgDir $inf) /install
        $pnputil = $LASTEXITCODE
        # 0 installed, 3010 installed (a reboot finishes it), 259 nothing
        # newer to install (the read-back says what runs).
        if (@(0, 3010, 259) -notcontains $pnputil) {
            throw "the NIC driver step: pnputil /add-driver $inf /install failed (exit $pnputil); '$alias' may have restarted; the instance was not touched"
        }
        # The adapter comes back with the new driver; Windows keeps a newer
        # one it ranks higher (remove that one first, on site).
        $deadline = (Get-Date).AddSeconds(60)
        do {
            Start-Sleep -Seconds 2
            $nowVersion = (Get-NetAdapter | Where-Object { $_.InterfaceGuid -eq $adapterGuid } | Select-Object -First 1).DriverVersion
        } while ($nowVersion -ne $wantVersion -and (Get-Date) -lt $deadline)
        if (-not (Get-NetAdapter | Where-Object { $_.InterfaceGuid -eq $adapterGuid })) {
            throw "the NIC driver step: no adapter has the Dante NIC's GUID $adapterGuid after pnputil /add-driver $inf /install (exit $pnputil): read the new one with Get-NetAdapter and set nic_guid in the PC profile; the instance was not touched"
        }
        if ($nowVersion -ne $wantVersion) {
            throw "the NIC driver step: '$alias' runs driver '$nowVersion' after pnputil /add-driver $inf /install (exit $pnputil), not the pinned $wantVersion (a newer driver Windows ranks higher stays: pnputil /delete-driver <its oem inf> /uninstall, on site, then run the install again); the instance was not touched"
        }
        Write-Output "the NIC driver step: '$alias' runs $inf $wantVersion"
    }
    & (Join-Path $here "dasio.exe") pc-profile apply
    $applied = $LASTEXITCODE
    if ($applied -eq 2) {
        throw "the baseline step: dasio pc-profile apply left must-have items unchecked or unset (the MUST-HAVE line above); dasio does not run without them; the instance was not touched"
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

# From here the instance is stopped. A failure from here on brings it back
# as it was (its binary, config and service command) and then reports the
# failure (dasio#28 A37/E20); a first install has nothing to bring back.
$instExe = Join-Path $bin $instExeName
$cfg = Join-Path $data "device.toml"
$before = Get-CimInstance Win32_Service -Filter "Name='$service'"
$rollback = Join-Path $data "rollback"
$beforeExe = $null
if ($before) {
    New-Item -ItemType Directory -Force $rollback | Out-Null
    $beforeExe = [regex]::Match($before.PathName, '^"([^"]+)"').Groups[1].Value
    if ($beforeExe -and (Test-Path $beforeExe)) { Copy-Item $beforeExe (Join-Path $rollback "instance.exe") -Force }
    if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $rollback "device.toml") -Force }
}
# Puts back `$Saved` over `$Path` when they differ.
function Restore-File([string]$Saved, [string]$Path) {
    if ((Test-Path $Saved) -and (-not (Test-Path $Path) -or (Get-FileHash $Path).Hash -ne (Get-FileHash $Saved).Hash)) {
        Copy-Item $Saved $Path -Force
    }
}
try {
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
    if (-not $dllHeld) {
        if (-not (Test-Path $protocolKey)) { New-Item -Path $protocolKey | Out-Null }
        New-ItemProperty -Path $protocolKey -Name ShmProtocol -Value $ShmProtocol -PropertyType DWord -Force | Out-Null
    }
} finally {
    foreach ($s in $paused) {
        Start-Service -Name $s.Name
        (Get-Service -Name $s.Name).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))
        Write-Output "$($s.Name) runs the new binary"
    }
}
# The process keeper (dasio#185): dasio-keep.exe, its own copy of dasio.exe
# (so a new dasio.exe never waits on it), run by the SYSTEM task
# dasio-pc-keep, holds every [[process]] entry's affinity and priority class
# (what Process Lasso did). The task starts it at boot and every 5 min, one
# at a time (a task's restart-on-failure covers only a failed start, not a
# keeper that ended), so a keeper that died is back within 5 min. Stopped
# for the copy and started again after it, also when a step fails.
$keepTask = "dasio-pc-keep"
$keepExe = Join-Path $bin "dasio-keep.exe"
try {
    if (Get-ScheduledTask -TaskName $keepTask -ErrorAction SilentlyContinue) {
        # Off first: its 5-min trigger must not start it again mid-copy.
        Disable-ScheduledTask -TaskName $keepTask | Out-Null
        Stop-ScheduledTask -TaskName $keepTask
    }
    # The task's stop may leave its process (CI: the copy found
    # dasio-keep.exe still in use right after it): end any keeper left.
    $left = @(Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue)
    if ($left.Count -gt 0) {
        Write-Output "ending the process keeper ($($left.Count) process(es) left after the task's stop)"
        # It may end on its own between the read and the stop: checked after.
        $left | Stop-Process -Force -ErrorAction SilentlyContinue
        @(Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue) | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
        if (Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue) { throw "the process keeper did not end within 15 s" }
    }
    # A scanner can hold a just-ended exe a moment: a few tries, then fail.
    for ($try = 1; ; $try++) {
        try { Copy-Item (Join-Path $here "dasio.exe") $keepExe -Force -ErrorAction Stop; break }
        catch { if ($try -ge 10) { throw "copying dasio-keep.exe failed 10 times: $($_.Exception.Message)" }; Start-Sleep -Milliseconds 500 }
    }
    $keepAction = New-ScheduledTaskAction -Execute $keepExe -Argument "pc-profile keep"
    $keepTriggers = @(
        (New-ScheduledTaskTrigger -AtStartup),
        (New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 5))
    )
    $keepSettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName $keepTask -Action $keepAction -Trigger $keepTriggers -Settings $keepSettings -User SYSTEM -RunLevel Highest -Force | Out-Null
} finally {
    if (Get-ScheduledTask -TaskName $keepTask -ErrorAction SilentlyContinue) {
        Enable-ScheduledTask -TaskName $keepTask | Out-Null
        Start-ScheduledTask -TaskName $keepTask
    }
}
# Running, and still running 2 s later: a keeper that ends at once is none.
$keepDeadline = (Get-Date).AddSeconds(10)
while (-not (Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue)) {
    if ((Get-Date) -gt $keepDeadline) { throw "the process keeper ($keepTask) did not start" }
    Start-Sleep -Milliseconds 500
}
Start-Sleep -Seconds 2
if (-not (Get-Process -Name "dasio-keep" -ErrorAction SilentlyContinue)) {
    throw "the process keeper ($keepTask) ended right after its start (see C:\ProgramData\dasio\pc-keep.log)"
}

# dtray, dstack's tray for every local dstack device (dasio's line per
# instance comes from its device's status). A running one holds its binary,
# which Windows lets move but not overwrite; moved-aside copies go once
# nothing runs them. It starts at the installing user's logon.
$dtrayCli = Join-Path $bin "dtray-cli.exe"
Get-ChildItem $bin -Filter "*.exe.old-*" -ErrorAction SilentlyContinue | ForEach-Object {
    Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue
}
# dtray-cli (dstack#218) has dtray's command-line modes as a console
# program, so `&` waits for it and sets $LASTEXITCODE; scripts call it.
foreach ($exe in "dtray.exe", "dtray-cli.exe") {
    $to = Join-Path $bin $exe
    $from = Join-Path $here $exe
    if (-not (Test-Path $to) -or (Get-FileHash $to).Hash -ne (Get-FileHash $from).Hash) {
        if (Test-Path $to) { Move-Item $to "$to.old-$(Get-Date -Format yyyyMMddHHmmss)" -Force }
        Copy-Item $from $to -Force
        Write-Output "installed $exe"
    }
}
# dtray itself starts at the next logon.
& $dtrayCli --autostart on
if ($LASTEXITCODE -ne 0) { throw "dtray-cli --autostart on failed ($LASTEXITCODE)" }
# The former dasio-tray: dtray shows its lines now.
$run = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"
if (Get-ItemProperty $run -Name "dasio-tray" -ErrorAction SilentlyContinue) { Remove-ItemProperty $run -Name "dasio-tray" }
Get-Process dasio-tray -ErrorAction SilentlyContinue | Stop-Process -Force
$oldTray = Join-Path $bin "dasio-tray.exe"
if (Test-Path $oldTray) { Move-Item $oldTray "$oldTray.old-$(Get-Date -Format yyyyMMddHHmmss)" -Force }
# dtray's list of devices this PC should run: one empty file per instance,
# named after its Dante device, so a stopped instance shows as expected.
$devices = Join-Path $env:ProgramData "dstack\devices"
New-Item -ItemType Directory -Force $devices | Out-Null
if ($deviceName) {
    New-Item -ItemType File -Force (Join-Path $devices "$deviceName.device") | Out-Null
    Set-Content (Join-Path $data "dtray-device") $deviceName
}
# The instance's own binary (its service is stopped).
if (-not (Test-Path $instExe) -or (Get-FileHash $instExe).Hash -ne (Get-FileHash (Join-Path $bin "dasio.exe")).Hash) {
    Copy-Item (Join-Path $bin "dasio.exe") $instExe -Force
}
# A re-install may be given the instance's own installed config: Copy-Item
# refuses to copy a file onto itself (dasio#148), and there is nothing to copy.
if ([string]::Equals($Config, [System.IO.Path]::GetFullPath($cfg), [System.StringComparison]::OrdinalIgnoreCase)) {
    Write-Output "the config is the instance's installed one ($cfg): kept as it is"
} else {
    Copy-Item $Config $cfg -Force
}

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
if (-not $TestClock) {
    # Power plan values are read when the plan is made active, QoS policies
    # at a Group Policy refresh: make both current now, with the instance
    # stopped (gpupdate's burst is no live audio's).
    & powercfg.exe /setactive SCHEME_CURRENT
    if ($LASTEXITCODE -ne 0) { Write-Output "WARNING: powercfg /setactive failed ($LASTEXITCODE): the power plan values take effect at the next reboot" }
    & gpupdate.exe /target:computer /force | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Output "WARNING: gpupdate failed ($LASTEXITCODE): the QoS policies (mDNS cap) take effect at the next reboot" }
    # The forced refresh can drop a policy value the apply above wrote: on
    # mbc (18.11.2026) it removed Windows Defender's DisableAntiSpyware
    # seconds after the apply had set it (dasio#24). Apply again, so the
    # install ends with the profile as it should be; it writes only what
    # drifted and needs no second refresh (the QoS values are unchanged).
    & (Join-Path $here "dasio.exe") pc-profile apply
    $applied = $LASTEXITCODE
    if ($applied -eq 2) {
        throw "the baseline step after the Group Policy refresh: dasio pc-profile apply left must-have items unset (the MUST-HAVE line above); dasio does not run without them, so the instance stays stopped: set them and run the install again"
    }
    if ($applied -ne 0) { Write-Output "the PC profile still has problems (listed above): see dasio#128" }
}
# A bridge instance (`[bridge]` in its config, dasio#106) carries Dante to a
# local ASIO driver itself and has no DAW side: registering dasio's own ASIO
# driver for it would list a driver no DAW can attach to.
$bridge = Select-String -LiteralPath $cfg -Pattern '^\s*\[bridge\]\s*(#.*)?$' -Quiet
if ($bridge) {
    Write-Output "instance $Instance is a bridge ([bridge] in its config): no dasio ASIO driver registered"
} else {
    $named = @()
    if ($Name) { $named = @("--name", $Name) }
    & $exe register --instance $Instance --dll (Join-Path $bin "dasio_driver.dll") @named
    if ($LASTEXITCODE -ne 0) { throw "register failed ($LASTEXITCODE)" }
}

$log = Join-Path $data "dasio.log"
$bin_path = "`"$instExe`" service --instance $Instance --config `"$cfg`" $place --log `"$log`""
if ($TestClock) { $bin_path += " --clock system" }
if ($DsyncClock) { $bin_path += " --clock dsync" }
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
} catch {
    $failure = $_.Exception.Message
    if (-not $before) { throw }
    try {
        if ($beforeExe) { Restore-File (Join-Path $rollback "instance.exe") $beforeExe }
        Restore-File (Join-Path $rollback "device.toml") $cfg
        $now = Get-CimInstance Win32_Service -Filter "Name='$service'"
        if ($now.PathName -ne $before.PathName) {
            $r = $now | Invoke-CimMethod -MethodName Change -Arguments @{ PathName = $before.PathName }
            if ($r.ReturnValue -ne 0) { throw "changing $service back failed ($($r.ReturnValue))" }
        }
        Start-Service -Name $service
        (Get-Service -Name $service).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))
    } catch {
        throw "install failed: $failure; rolling instance $Instance back failed too: $($_.Exception.Message)"
    }
    throw "install failed: $failure; instance $Instance rolled back and running as before"
}
Remove-Item -Recurse -Force $rollback -ErrorAction SilentlyContinue

Write-Output "dasio instance $Instance installed and started ($instExeName)"
