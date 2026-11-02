# dasio NDIS send pacer: print each bound adapter's pacer state
# (IOCTL_DASIO_PACER_STATUS). Run as administrator. -Every <s> repeats.
param([double]$Every = 0, [int]$Count = 1)
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class DasioPacer {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, IntPtr inBuf, uint inLen, byte[] outBuf, uint outLen, out uint got, IntPtr ov);
    // CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD = 0x17, 14, METHOD_BUFFERED, FILE_ANY_ACCESS)
    const uint IOCTL_DASIO_PACER_STATUS = (0x17u << 16) | (14u << 2);
    public static ulong[] Read() {
        using (var h = CreateFile(@"\\.\dasiopacer", 0xC0000000u, 3u, IntPtr.Zero, 3u, 0u, IntPtr.Zero)) {
            if (h.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "open \\\\.\\dasiopacer");
            var buf = new byte[8 * 24 * 16];
            uint got;
            if (!DeviceIoControl(h, IOCTL_DASIO_PACER_STATUS, IntPtr.Zero, 0, buf, (uint)buf.Length, out got, IntPtr.Zero))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "IOCTL_DASIO_PACER_STATUS");
            var v = new ulong[got / 8];
            Buffer.BlockCopy(buf, 0, v, 0, (int)got);
            return v;
        }
    }
}
"@

# The fields of DASIO_PACER_STATUS (filteruser.h), in order.
$names = "Version", "IfIndex", "State", "LinkAtAttach", "LinkLast", "LinkUpdates", "Rate", "Burst", "Tokens", "Last", "Now",
    "Queued", "QueuedBytes", "RingCount", "MaxQueued", "PrioritySent", "Admitted", "Deferred", "TimerArms", "TimerFires",
    "TimerArmed", "Pumps", "PumpSent", "Pumping"
for ($n = 0; $n -lt $Count; $n++) {
    $v = [DasioPacer]::Read()
    if ($v.Length % $names.Count -ne 0) { throw "status of $($v.Length * 8) bytes is not a whole number of records" }
    for ($i = 0; $i -lt $v.Length; $i += $names.Count) {
        if ($v[$i] -ne 1) { throw "status version $($v[$i]), this script reads 1" }
        $o = [ordered]@{ Time = (Get-Date -Format o) }
        for ($k = 0; $k -lt $names.Count; $k++) {
            $o[$names[$k]] = if ($names[$k] -eq "Tokens") { [BitConverter]::ToInt64([BitConverter]::GetBytes($v[$i + $k]), 0) } else { $v[$i + $k] }
        }
        [pscustomobject]$o
    }
    if ($n + 1 -lt $Count) { Start-Sleep -Milliseconds ([int]($Every * 1000)) }
}
