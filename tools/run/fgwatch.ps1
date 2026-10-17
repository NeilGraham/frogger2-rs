# tools/run/fgwatch.ps1: read-only desktop-disruption watcher for oracle runs.
#
#   powershell -File fgwatch.ps1 -Prefix C:\f2-runs\<id>\ -Seconds 60 [-IntervalMs 1000] [-StopFile F]
#
# Every -IntervalMs it samples, with read-only Win32 queries only: the
# foreground window's process, the cursor position and clip rectangle, the
# primary display mode, and every top-level window of each frogger2.exe whose
# path starts with -Prefix (class, visible, rectangle, ex-style, on a monitor).
# One JSON object per line on stdout. Nothing is captured, activated, moved
# or clicked.
param(
  [Parameter(Mandatory = $true)][string]$Prefix,
  [int]$Seconds = 60,
  [int]$IntervalMs = 2000,
  [string]$StopFile = ''
)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public struct FgwRect { public int L, T, R, B; }
public struct FgwPoint { public int X, Y; }
public static class Fgw {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out FgwPoint p);
  [DllImport("user32.dll")] public static extern bool GetClipCursor(out FgwRect r);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out FgwRect r);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, byte[] dm);
  public static int[] Mode() {
    var dm = new byte[220];
    BitConverter.GetBytes((short)220).CopyTo(dm, 68);
    if (!EnumDisplaySettings(null, -1, dm)) return new int[0];
    return new int[] { BitConverter.ToInt32(dm, 172), BitConverter.ToInt32(dm, 176),
                       BitConverter.ToInt32(dm, 168), BitConverter.ToInt32(dm, 184) };
  }
  public static List<string> Windows(HashSet<uint> pids) {
    var o = new List<string>();
    int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77);
    int vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
    EnumWindows((h, l) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (!pids.Contains(pid)) return true;
      FgwRect r; GetWindowRect(h, out r);
      var sb = new StringBuilder(64); GetClassName(h, sb, 64);
      bool vis = IsWindowVisible(h);
      bool on = vis && r.R > vx && r.L < vx + vw && r.B > vy && r.T < vy + vh;
      o.Add(String.Format("{{\"pid\":{0},\"class\":\"{1}\",\"visible\":{2},\"rect\":[{3},{4},{5},{6}],\"exstyle\":{7},\"on_screen\":{8}}}",
        pid, sb.ToString().Replace("\"", "'"), vis ? "true" : "false", r.L, r.T, r.R, r.B,
        GetWindowLong(h, -20), on ? "true" : "false"));
      return true;
    }, IntPtr.Zero);
    return o;
  }
}
'@
$t0 = Get-Date
while ($true) {
  $t = ((Get-Date) - $t0).TotalSeconds
  $tp = @(Get-Process -Name frogger2 -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($Prefix, [StringComparison]::OrdinalIgnoreCase) })
  $pids = New-Object 'System.Collections.Generic.HashSet[uint32]'
  foreach ($p in $tp) { [void]$pids.Add([uint32]$p.Id) }
  $fg = [Fgw]::GetForegroundWindow()
  $fgpid = [uint32]0
  [void][Fgw]::GetWindowThreadProcessId($fg, [ref]$fgpid)
  $fgname = ''
  if ($fgpid -ne 0) {
    $fp = Get-Process -Id $fgpid -ErrorAction SilentlyContinue
    $fgname = $fp.ProcessName
  }
  $cp = New-Object FgwPoint; [void][Fgw]::GetCursorPos([ref]$cp)
  $cr = New-Object FgwRect; [void][Fgw]::GetClipCursor([ref]$cr)
  $mode = [Fgw]::Mode()
  $wins = [Fgw]::Windows($pids)
  $fggame = $pids.Contains($fgpid)
  '{{"t":{0},"fg_pid":{1},"fg_proc":"{2}","fg_is_game":{3},"cursor":[{4},{5}],"clip":[{6},{7},{8},{9}],"mode":[{10}],"game_count":{11},"game_windows":[{12}]}}' -f `
    ([math]::Round($t, 1)).ToString([Globalization.CultureInfo]::InvariantCulture), $fgpid, $fgname,
    $(if ($fggame) { 'true' } else { 'false' }), $cp.X, $cp.Y, $cr.L, $cr.T, $cr.R, $cr.B,
    ($mode -join ','), $tp.Count, ($wins -join ',')
  if ($t -ge $Seconds) { break }
  if ($StopFile -and (Test-Path -LiteralPath $StopFile)) { break }
  Start-Sleep -Milliseconds $IntervalMs
}
