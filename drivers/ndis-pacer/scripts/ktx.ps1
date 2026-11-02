# dasio NDIS filter: the kernel test stream (dasio#1). The filter builds UDP
# frames itself and hands them straight to the miniport, below tcpip and WFP,
# one every -IntervalUs for -DurationS seconds. The payload starts with the
# sequence number and the send time (100 ns), both 8-byte big endian.
# Run as administrator.
#   .\ktx.ps1 start -Adapter Ethernet -DstMac 00-11-22-33-44-55 -DstIp 10.0.0.2 -DstPort 47000
#   .\ktx.ps1 status
#   .\ktx.ps1 stop
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet("start", "stop", "status")][string]$Action,
    [string]$Adapter = "Ethernet",
    [string]$DstMac,
    [string]$DstIp,
    [ValidateRange(1, 65535)][int]$DstPort = 47000,
    [ValidateRange(1, 65535)][int]$SrcPort = 47000,
    [ValidateRange(0, 63)][int]$Dscp = 46,
    [ValidateRange(16, 1472)][int]$PayloadBytes = 200,
    [ValidateRange(50, 1000000)][int]$IntervalUs = 250,
    [ValidateRange(1, 86400)][int]$DurationS = 600
)
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class DasioKtx {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] inBuf, uint inLen, byte[] outBuf, uint outLen, out uint got, IntPtr ov);
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD = 0x17, n, METHOD_BUFFERED, FILE_ANY_ACCESS)
    public const uint START = (0x17u << 16) | (16u << 2);
    public const uint STOP = (0x17u << 16) | (17u << 2);
    public const uint STATUS = (0x17u << 16) | (18u << 2);
    public static byte[] Call(uint code, byte[] input, int outLen) {
        using (var h = CreateFile(@"\\.\dasiopacer", 0xC0000000u, 3u, IntPtr.Zero, 3u, 0u, IntPtr.Zero)) {
            if (h.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "open \\\\.\\dasiopacer");
            var output = new byte[outLen];
            uint got;
            if (!DeviceIoControl(h, code, input, input == null ? 0u : (uint)input.Length, output, (uint)outLen, out got, IntPtr.Zero))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "IOCTL " + code.ToString("x"));
            var r = new byte[got];
            Array.Copy(output, r, got);
            return r;
        }
    }
}
"@

switch ($Action) {
    "start" {
        if (-not $DstMac -or -not $DstIp) { throw "start needs -DstMac and -DstIp" }
        $nic = Get-NetAdapter -Name $Adapter
        $src = (Get-NetIPAddress -InterfaceIndex $nic.ifIndex -AddressFamily IPv4 | Select-Object -First 1).IPAddress
        $mac = $DstMac -split "[-:]" | ForEach-Object { [Convert]::ToByte($_, 16) }
        if ($mac.Count -ne 6) { throw "-DstMac needs 6 bytes" }
        # DASIO_KTX_START (filteruser.h), 40 bytes with its padding.
        $b = New-Object byte[] 40
        [BitConverter]::GetBytes([uint32]$nic.ifIndex).CopyTo($b, 0)
        ([byte[]]$mac).CopyTo($b, 4)
        ([System.Net.IPAddress]::Parse($DstIp)).GetAddressBytes().CopyTo($b, 10)
        ([System.Net.IPAddress]::Parse($src)).GetAddressBytes().CopyTo($b, 14)
        [BitConverter]::GetBytes([uint16]$DstPort).CopyTo($b, 18)
        [BitConverter]::GetBytes([uint16]$SrcPort).CopyTo($b, 20)
        $b[22] = [byte]$Dscp
        [BitConverter]::GetBytes([uint32]$PayloadBytes).CopyTo($b, 28)
        [BitConverter]::GetBytes([uint32]$IntervalUs).CopyTo($b, 32)
        [BitConverter]::GetBytes([uint32]$DurationS).CopyTo($b, 36)
        [void][DasioKtx]::Call([DasioKtx]::START, $b, 0)
        Write-Output "ktx started on $Adapter (ifIndex $($nic.ifIndex)) $src -> $DstIp`:$DstPort, $PayloadBytes B every $IntervalUs us for $DurationS s, DSCP $Dscp"
    }
    "stop" {
        [void][DasioKtx]::Call([DasioKtx]::STOP, $null, 0)
        Write-Output "ktx stopped"
    }
    "status" {
        # The fields of DASIO_KTX_STATUS (filteruser.h), in order.
        $names = "Version", "IfIndex", "Active", "Ready", "Start", "End", "Now", "IntervalUs", "Sent", "Completed",
            "Failed", "Out", "NoSlot", "Fires", "MaxBatch", "MaxLate"
        $raw = [DasioKtx]::Call([DasioKtx]::STATUS, $null, 8 * 16 * 16)
        $v = New-Object UInt64[] ($raw.Length / 8)
        [Buffer]::BlockCopy($raw, 0, $v, 0, $raw.Length)
        if ($v.Length % $names.Count -ne 0) { throw "status of $($raw.Length) bytes is not a whole number of records" }
        for ($i = 0; $i -lt $v.Length; $i += $names.Count) {
            $o = [ordered]@{}
            for ($j = 0; $j -lt $names.Count; $j++) { $o[$names[$j]] = $v[$i + $j] }
            [pscustomobject]$o
        }
    }
}
