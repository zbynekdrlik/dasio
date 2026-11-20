# dasio NDIS filter: kernel RX rings (dasio#27, B23). Each open ring: the
# adapter, whether it is attached (0 after a pause or detach of the filter:
# the reader reopens), its mode (Flags 1 = steal), slots, match entries and
# what it counted (status version 2: Tagged and BadChecksum last). Nothing
# when no ring is open (RxRingEnable 0, the default, or no reader), or on an
# older filter. Run as administrator.
#   .\rxring.ps1 status
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet("status")][string]$Action
)
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class DasioRxRing {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] inBuf, uint inLen, byte[] outBuf, uint outLen, out uint got, IntPtr ov);
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD = 0x17, 26, METHOD_BUFFERED, FILE_ANY_ACCESS)
    public const uint STATUS = (0x17u << 16) | (26u << 2);
    public static byte[] Call(uint code, int outLen) {
        using (var h = CreateFile(@"\\.\dasiopacer", 0xC0000000u, 3u, IntPtr.Zero, 3u, 0u, IntPtr.Zero)) {
            if (h.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "open \\\\.\\dasiopacer");
            var output = new byte[outLen];
            uint got;
            if (!DeviceIoControl(h, code, null, 0u, output, (uint)outLen, out got, IntPtr.Zero))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "IOCTL " + code.ToString("x"));
            var r = new byte[got];
            Array.Copy(output, r, got);
            return r;
        }
    }
}
"@

switch ($Action) {
    "status" {
        # The fields of DASIO_RX_STATUS (filteruser.h), in order.
        $names = "Version", "IfIndex", "Attached", "Flags", "SlotCount", "Matches", "Matched", "Delivered", "Full", "Stolen", "TooBig", "Tagged", "BadChecksum"
        $raw = [DasioRxRing]::Call([DasioRxRing]::STATUS, 8 * $names.Count * 64)
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
