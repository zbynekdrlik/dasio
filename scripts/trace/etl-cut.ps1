# dasio#207 (25.11.2026): cut one moment out of a kernel trace (.etl) without tracerpt (3.8 MB/s of CSV, 14 min for
# 231 MB). Reads the trace once through ETW's own consumer API and writes, as compact rows:
#   the window  -PreMs .. +PostMs around -AtUtc:
#     C,t,cpu,newTid,oldTid,newPri,oldPri,oldWaitReason,oldState,newWaitTime      context switch
#     R,t,cpu,tid,byTid,byPid,adjustReason,adjustIncrement                         a thread made ready (by whom)
#     S,t,cpu,tid,ip                                                              1 ms sample
#     I,t,cpu,routine   D,t,cpu,routine,opcode                                     ISR / DPC (time = its end)
#     K,t,cpu,pid,tid,stamp,addr;addr;...   a stack (a trace with <Stacks>), only of a thread whose last wait was
#                                           on a kernel resource (WrResource); kernel and user part share `stamp`
#   the whole trace:  T,op,pid,tid,startAddr,basePri,name   P,op,pid,parent,image   M,op,pid,base,size,file
# t is in 100 ns steps from -AtUtc. The summary (-Out + '.sum.txt') has every CPU's event count, bytes and how far
# back its ring reaches. BelowNormal on CPUs 1-3.
#
# Call:  powershell -File etl-cut.ps1 -Etl C:\ProgramData\dasio\tmp\csw.etl -Out csw.cut.txt -AtUtc 2026-11-25T01:22:13Z
# Read the cut with csw_an.py, csw_res.py, csw_smp.py, csw_stk.py, csw_dump.py (same folder).
param([Parameter(Mandatory)][string]$Etl, [Parameter(Mandatory)][string]$Out, [Parameter(Mandatory)][string]$AtUtc, [int]$PreMs = 1500, [int]$PostMs = 500)
$ErrorActionPreference = 'Stop'
$me = Get-Process -Id $PID; $me.PriorityClass = 'BelowNormal'; $me.ProcessorAffinity = 0xE
$sum = "$Out.sum.txt"
"start $((Get-Date).ToUniversalTime().ToString('HH:mm:ss')) $Etl at $AtUtc" | Set-Content $sum
try {
Add-Type -TypeDefinition @'
using System; using System.IO; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
namespace Dasio {
public static class EtlCut {
  [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate void RecCb(IntPtr rec);
  [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern ulong OpenTraceW(IntPtr logfile);
  [DllImport("advapi32.dll", SetLastError = true)] static extern uint ProcessTrace(ulong[] handles, uint count, IntPtr start, IntPtr end);
  [DllImport("advapi32.dll")] static extern uint CloseTrace(ulong handle);
  static StreamWriter w; static long lo, hi, at, n, kept, stacks;
  static long first = long.MaxValue, last = long.MinValue;
  static long[] cpuFirst = new long[256], cpuCount = new long[256], cpuBytes = new long[256];
  static Dictionary<uint, int> lastWait = new Dictionary<uint, int>();
  static string Ansi(IntPtr p, int off, int end) { StringBuilder s = new StringBuilder(); for (int i = off; i < end; i++) { byte b = Marshal.ReadByte(p, i); if (b == 0) break; s.Append((char)b); } return s.ToString(); }
  static string Wide(IntPtr p, int off, int end) { StringBuilder s = new StringBuilder(); for (int i = off; i + 1 < end; i += 2) { char c = (char)Marshal.ReadInt16(p, i); if (c == 0) break; s.Append(c == ',' ? ';' : c); } return s.ToString(); }
  static void On(IntPtr r) {
    n++;
    long ts = Marshal.ReadInt64(r, 16);
    int g = Marshal.ReadInt32(r, 24);
    byte op = Marshal.ReadByte(r, 45);
    int cpu = Marshal.ReadByte(r, 80);
    int len = (ushort)Marshal.ReadInt16(r, 86);
    IntPtr d = Marshal.ReadIntPtr(r, 96);
    if (ts < first) first = ts; if (ts > last) last = ts;
    if (cpuCount[cpu]++ == 0) cpuFirst[cpu] = ts;
    cpuBytes[cpu] += len + 24;
    bool inWin = ts >= lo && ts <= hi;
    if (g == unchecked((int)0x3d6fa8d1)) {
      if (op == 36) {
        if (len < 24) return;
        uint oldTid = (uint)Marshal.ReadInt32(d, 4);
        lastWait[oldTid] = Marshal.ReadByte(d, 14) == 5 ? (sbyte)Marshal.ReadByte(d, 12) : -1;
        if (!inWin) return; kept++;
        w.Write("C,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 0)); w.Write(','); w.Write(oldTid);
        w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 8)); w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 9)); w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 12));
        w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 14)); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 16)); w.Write('\n');
      } else if (op == 50) {
        if (!inWin || len < 8) return; kept++;
        w.Write("R,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 0)); w.Write(','); w.Write((uint)Marshal.ReadInt32(r, 8));
        w.Write(','); w.Write((uint)Marshal.ReadInt32(r, 12)); w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 4)); w.Write(','); w.Write((sbyte)Marshal.ReadByte(d, 5)); w.Write('\n');
      } else if (op >= 1 && op <= 4 && len >= 72) {
        w.Write("T,"); w.Write(op); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 0)); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 4)); w.Write(','); w.Write(Marshal.ReadInt64(d, 48).ToString("X"));
        w.Write(','); w.Write(Marshal.ReadByte(d, 68)); w.Write(','); w.Write(len > 74 ? Wide(d, 72, len) : ""); w.Write('\n');
      }
    } else if (g == unchecked((int)0xdef2fe46)) {
      if (op != 32 || len < 24) return;
      stacks++;
      if (!inWin) return;
      uint tid = (uint)Marshal.ReadInt32(d, 12); int why;
      if (!lastWait.TryGetValue(tid, out why) || why != 27) return;
      kept++;
      w.Write("K,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 8)); w.Write(','); w.Write(tid); w.Write(','); w.Write(Marshal.ReadInt64(d, 0)); w.Write(',');
      for (int o = 16; o + 8 <= len; o += 8) { if (o > 16) w.Write(';'); w.Write(Marshal.ReadInt64(d, o).ToString("X")); }
      w.Write('\n');
    } else if (g == unchecked((int)0xce1dbfb4)) {
      if (!inWin) return;
      if (op == 46 && len >= 12) { kept++; w.Write("S,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 8)); w.Write(','); w.Write(Marshal.ReadInt64(d, 0).ToString("X")); w.Write('\n'); }
      else if ((op == 67 || op == 50) && len >= 16) { kept++; w.Write("I,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write(Marshal.ReadInt64(d, 8).ToString("X")); w.Write('\n'); }
      else if ((op == 66 || op == 68 || op == 69) && len >= 16) { kept++; w.Write("D,"); w.Write(ts - at); w.Write(','); w.Write(cpu); w.Write(','); w.Write(Marshal.ReadInt64(d, 8).ToString("X")); w.Write(','); w.Write(op); w.Write('\n'); }
    } else if (g == unchecked((int)0x2cb15d1d)) {
      if ((op == 10 || op == 2 || op == 3 || op == 4) && len > 58) {
        w.Write("M,"); w.Write(op); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 16)); w.Write(','); w.Write(Marshal.ReadInt64(d, 0).ToString("X")); w.Write(','); w.Write(Marshal.ReadInt64(d, 8).ToString("X"));
        w.Write(','); w.Write(Wide(d, 56, len)); w.Write('\n');
      }
    } else if (g == unchecked((int)0x3d6fa8d0)) {
      if (op >= 1 && op <= 4 && len > 44) {
        string image = "?";
        try { int off = 36; if (Marshal.ReadInt32(d, off) == 0) off += 4; else { off += 16; off += 8 + 4 * Marshal.ReadByte(d, off + 1); } if (off < len) image = Ansi(d, off, len); } catch (Exception) { }
        w.Write("P,"); w.Write(op); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 8)); w.Write(','); w.Write((uint)Marshal.ReadInt32(d, 12)); w.Write(','); w.Write(image); w.Write('\n');
      }
    }
  }
  public static string Run(string etl, string outPath, long atFiletime, long preTicks, long postTicks) {
    at = atFiletime; lo = at - preTicks; hi = at + postTicks;
    IntPtr lf = Marshal.AllocHGlobal(1024); for (int i = 0; i < 1024; i += 8) Marshal.WriteInt64(lf, i, 0);
    Marshal.WriteIntPtr(lf, 0, Marshal.StringToHGlobalUni(etl));
    Marshal.WriteInt32(lf, 28, 0x10000000);
    RecCb cb = new RecCb(On); GCHandle keep = GCHandle.Alloc(cb);
    Marshal.WriteIntPtr(lf, 424, Marshal.GetFunctionPointerForDelegate(cb));
    ulong h = OpenTraceW(lf);
    if (h == ulong.MaxValue || h == 0xFFFFFFFFUL) return "OpenTrace failed, error " + Marshal.GetLastWin32Error();
    StringBuilder s = new StringBuilder();
    s.Append("header: cpus=" + Marshal.ReadInt32(lf, 132) + " pointer=" + Marshal.ReadInt32(lf, 164) + " perf_freq=" + Marshal.ReadInt64(lf, 376) + " events_lost=" + Marshal.ReadInt32(lf, 168) + " buffers_lost=" + Marshal.ReadInt32(lf, 396) + "\n");
    w = new StreamWriter(outPath, false, new UTF8Encoding(false), 1 << 20);
    uint rc = ProcessTrace(new ulong[] { h }, 1, IntPtr.Zero, IntPtr.Zero);
    CloseTrace(h); w.Flush(); w.Close(); keep.Free();
    s.Append("ProcessTrace rc=" + rc + " events=" + n + " stacks=" + stacks + " window_rows=" + kept + "\n");
    if (n > 0) {
      s.Append("first " + DateTime.FromFileTimeUtc(first).ToString("HH:mm:ss.fff") + " last " + DateTime.FromFileTimeUtc(last).ToString("HH:mm:ss.fff") + "\n");
      for (int c = 0; c < 256; c++) if (cpuCount[c] > 0) s.Append("cpu " + c + " events " + cpuCount[c] + " kib " + (cpuBytes[c] / 1024) + " reaches_back_s " + ((last - cpuFirst[c]) / 10000000.0).ToString("F1", System.Globalization.CultureInfo.InvariantCulture) + "\n");
    }
    return s.ToString();
  }
}
}
'@
  $styles = [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal
  $at = [DateTime]::Parse($AtUtc, [Globalization.CultureInfo]::InvariantCulture, $styles).ToFileTimeUtc()
  $r = [Dasio.EtlCut]::Run($Etl, $Out, $at, [int64]$PreMs * 10000, [int64]$PostMs * 10000)
  $r | Add-Content $sum
  "out $((Get-Item $Out).Length) bytes" | Add-Content $sum
  "done $((Get-Date).ToUniversalTime().ToString('HH:mm:ss'))" | Add-Content $sum
} catch { "FAILED: $_" | Add-Content $sum }
