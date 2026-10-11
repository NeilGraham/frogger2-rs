# Screenshot + key harness for ares (N64), adapted from C:\f2-ps5test\shot.ps1.
#   ares-shot.ps1 -rom C:\f2-n64test\frogger2.z64 -out C:\f2-n64test\a.png [-delay 8] [-shots 1]
#                 [-gap 3] [-keys "Return;Up,X"] [-hold 400] [-log C:\f2-n64test\run.txt]
#                 [-settings C:\f2-n64test\settings.bml]
# Takes `shots` captures of the ares window's client area (PrintWindow) (a.png, a_1.png, ...) `gap` seconds
# apart; before capture i the keys of step i (comma separated System.Windows.Forms.Keys
# names, held for -hold ms, default 400; slow ROMs need longer) are pressed. ares' stdout (ISViewer = libdragon debugf, with
# HomebrewMode on) goes to -log.
param([string]$rom, [string]$out, [int]$delay = 8, [int]$shots = 1, [int]$gap = 3, [string]$keys = "", [int]$hold = 400,
      [string]$log = "C:\f2-n64test\run.txt", [string]$settings = "C:\f2-n64test\settings.bml")
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public class W { [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 public struct RECT { public int L, T, R, B; }
 public struct POINT { public int X, Y; }
 public delegate bool EnumProc(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
 [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
 [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint type);
 public static void Down(byte vk) { byte sc = (byte)MapVirtualKey(vk, 0); uint ext = (vk >= 0x21 && vk <= 0x28) ? 1u : 0u; keybd_event(vk, sc, ext, UIntPtr.Zero); }
 public static void Up(byte vk) { byte sc = (byte)MapVirtualKey(vk, 0); uint ext = (vk >= 0x21 && vk <= 0x28) ? 1u : 0u; keybd_event(vk, sc, ext | 2u, UIntPtr.Zero); }
 public static IntPtr FindByPid(uint pid) { IntPtr found = IntPtr.Zero; int best = 0;
  EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
   if (p == pid && IsWindowVisible(h)) { RECT r; GetWindowRect(h, out r); int a = (r.R - r.L) * (r.B - r.T);
    if (a > best) { best = a; found = h; } } return true; }, IntPtr.Zero);
  return found; } }
"@
[W]::SetProcessDPIAware() | Out-Null
$exe = "C:\Users\<you>\Emulation\emulators\ares-v148\ares.exe"
$args = @('--settings-file', "`"$settings`"", '--system', '"Nintendo 64"', '--no-file-prompt', "`"$rom`"")
$p = Start-Process -FilePath $exe -ArgumentList $args -PassThru -RedirectStandardOutput $log -RedirectStandardError ($log + ".err") -WorkingDirectory "C:\f2-n64test"
Start-Sleep -Seconds $delay
$h = [W]::FindByPid([uint32]$p.Id)
$sb = New-Object System.Text.StringBuilder 256
[W]::GetWindowText($h, $sb, 256) | Out-Null
Write-Output ("window: " + $sb.ToString())
[W]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 500
# PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT) renders the ares window itself, so other
# windows covering it are never captured.
function Grab([string]$file) {
  $r = New-Object W+RECT; [W]::GetClientRect($h, [ref]$r) | Out-Null
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  if ($w -le 0 -or $hh -le 0) { Write-Output "no window"; return }
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  $ok = [W]::PrintWindow($h, $hdc, 3)
  $g.ReleaseHdc($hdc)
  if (-not $ok) { Write-Output "PrintWindow failed"; return }
  $bmp.Save($file)
  Write-Output "saved $file ($w x $hh)"
}
Grab $out
$steps = $keys.Split(';')
for ($i = 1; $i -lt $shots; $i++) {
  if ($i - 1 -lt $steps.Length) {
    $ks = @($steps[$i - 1].Split(',') | Where-Object { $_ -ne '' } | ForEach-Object { [byte][System.Windows.Forms.Keys]::$_ })
    if ($ks.Length -gt 0) {
      [W]::SetForegroundWindow($h) | Out-Null
      foreach ($k in $ks) { [W]::Down($k) }
      Start-Sleep -Milliseconds $hold
      Grab ($out.Replace('.png', "_${i}held.png"))
      foreach ($k in $ks) { [W]::Up($k) }
    }
  }
  Start-Sleep -Seconds $gap
  Grab ($out.Replace('.png', "_$i.png"))
}
Stop-Process -Id $p.Id -Force
