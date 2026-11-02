# dasio NDIS send pacer: set, live for all bound adapters, the share of the
# link other traffic gets and its burst (IOCTL_DASIO_PACER_SET). Run as
# administrator. -RatePercent 0 = no pacing; -BurstBytes 0 = one timer tick.
# Not kept across a restart of the filter (the defaults come back).
param(
    [Parameter(Mandatory)][ValidateRange(0, 100)][int]$RatePercent,
    [Parameter(Mandatory)][ValidateRange(0, 16777216)][int]$BurstBytes
)
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class DasioPacerSet {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] inBuf, uint inLen, IntPtr outBuf, uint outLen, out uint got, IntPtr ov);
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD = 0x17, 15, METHOD_BUFFERED, FILE_ANY_ACCESS)
    const uint IOCTL_DASIO_PACER_SET = (0x17u << 16) | (15u << 2);
    public static void Set(uint ratePercent, uint burstBytes) {
        using (var h = CreateFile(@"\\.\dasiopacer", 0xC0000000u, 3u, IntPtr.Zero, 3u, 0u, IntPtr.Zero)) {
            if (h.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "open \\\\.\\dasiopacer");
            var buf = new byte[8];
            BitConverter.GetBytes(ratePercent).CopyTo(buf, 0);
            BitConverter.GetBytes(burstBytes).CopyTo(buf, 4);
            uint got;
            if (!DeviceIoControl(h, IOCTL_DASIO_PACER_SET, buf, (uint)buf.Length, IntPtr.Zero, 0, out got, IntPtr.Zero))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "IOCTL_DASIO_PACER_SET");
        }
    }
}
"@

[DasioPacerSet]::Set([uint32]$RatePercent, [uint32]$BurstBytes)
Write-Output "pacer set: $RatePercent % of the link, burst $BurstBytes B (0 = one tick)"
