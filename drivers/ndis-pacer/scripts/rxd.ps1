# dasio NDIS filter: deferred receive (dasio#1). The NIC's receive DPC hands
# its packets to the filter and returns at once; a DPC of the filter's own,
# on -Cpu, indicates them up, so tcpip's and WFP's work for them runs there
# and not inside the NIC's DPC. Only TCP and discovery chatter (mDNS, LLMNR,
# NetBIOS, SSDP, broadcast) are deferred; Dante audio, PTP and the rest go
# up at once. Run as administrator. Off after a restart of
# the filter.
#   .\rxd.ps1 on -Cpu 11
#   .\rxd.ps1 off
#   .\rxd.ps1 status
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet("on", "off", "status")][string]$Action,
    [ValidateRange(-1, 1023)][int]$Cpu = -1
)
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class DasioRxd {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] inBuf, uint inLen, byte[] outBuf, uint outLen, out uint got, IntPtr ov);
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD = 0x17, n, METHOD_BUFFERED, FILE_ANY_ACCESS)
    public const uint SET = (0x17u << 16) | (19u << 2);
    public const uint STATUS = (0x17u << 16) | (20u << 2);
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
    { $_ -in "on", "off" } {
        # DASIO_RXD_SET (filteruser.h): Enable, Cpu.
        $b = New-Object byte[] 8
        [BitConverter]::GetBytes([uint32]($(if ($Action -eq "on") { 1 } else { 0 }))).CopyTo($b, 0)
        [BitConverter]::GetBytes([int32]$Cpu).CopyTo($b, 4)
        [void][DasioRxd]::Call([DasioRxd]::SET, $b, 0)
        Write-Output "deferred receive $Action (DPC CPU $Cpu)"
    }
    "status" {
        # The fields of DASIO_RXD_STATUS (filteruser.h), in order.
        $names = "Version", "IfIndex", "Enabled", "Cpu", "Deferred", "Inline", "DpcRuns", "MaxBatch", "Queued", "Dropped", "Priority"
        $raw = [DasioRxd]::Call([DasioRxd]::STATUS, $null, 8 * 11 * 16)
        $v = New-Object Int64[] ($raw.Length / 8)
        [Buffer]::BlockCopy($raw, 0, $v, 0, $raw.Length)
        if ($v.Length % $names.Count -ne 0) { throw "status of $($raw.Length) bytes is not a whole number of records" }
        for ($i = 0; $i -lt $v.Length; $i += $names.Count) {
            $o = [ordered]@{}
            for ($j = 0; $j -lt $names.Count; $j++) { $o[$names[$j]] = $v[$i + $j] }
            [pscustomobject]$o
        }
    }
}
