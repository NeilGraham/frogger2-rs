# Screenshot + key harness for ares (N64), adapted from C:\f2-ps5test\shot.ps1.
#   ares-shot.ps1 -rom C:\f2-n64test\frogger2.z64 -out C:\f2-n64test\a.png [-delay 8] [-shots 1]
#                 [-gap 3] [-log C:\f2-n64test\run.txt]
#                 [-settings C:\f2-n64test\settings.bml] [-stopon "mem-tour done"]
# Takes `shots` captures of the ares window's client area (PrintWindow) (a.png, a_1.png, ...) `gap` seconds
# apart. ares' stdout (ISViewer = libdragon debugf, with HomebrewMode on) goes to -log.
# The log is watched during -delay: a crash (libdragon's inspector: ASSERTION FAILED, CPU
# EXCEPTION, RUST PANIC) ends the run at once with a capture of the crash screen
# (<out>-crash.png, taken 2 s later so the inspector has drawn) and exit code 3; a line
# matching -stopon (regex) ends it early with exit code 0 (e.g. a test boot's "done" line).
# Never touches the user's desktop: the ares window is not brought to the foreground (it runs
# unfocused, Input/Defocus = Block) and no keys are sent. -keys is refused: keybd_event
# injects system-wide keystrokes into whatever window has focus (it typed into the user's
# work windows). Drive the ROM from inside instead (F2N64_BOOT test modes, tools/n64/README.md).
param([string]$rom, [string]$out, [int]$delay = 8, [int]$shots = 1, [int]$gap = 3, [string]$keys = "", [int]$hold = 400,
      [string]$log = "C:\f2-n64test\run.txt", [string]$settings = "C:\f2-n64test\settings.bml",
      [string]$stopon = "")
if ($keys -ne "") { Write-Error "-keys is disabled (system-wide keystrokes reach the user's windows); use an F2N64_BOOT test mode"; exit 2 }
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class W { [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
 public struct RECT { public int L, T, R, B; }
 public struct POINT { public int X, Y; }
 public delegate bool EnumProc(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
 [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
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
$crashRe = 'ASSERTION FAILED|CPU EXCEPTION|RUST PANIC'
$stopped = ""
$t0 = Get-Date
while (((Get-Date) - $t0).TotalSeconds -lt $delay -and -not $p.HasExited) {
  Start-Sleep -Milliseconds 1000
  try {
    $fs = [System.IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
    $text = (New-Object System.IO.StreamReader($fs)).ReadToEnd(); $fs.Close()
  } catch { continue }
  $m = [regex]::Match($text, "(?m)^.*($crashRe).*$")
  if ($m.Success) { $stopped = "crash"; Write-Output ("CRASH after {0:N0} s: {1}" -f ((Get-Date) - $t0).TotalSeconds, $m.Value.Trim()); Start-Sleep -Seconds 2; break }
  if ($stopon -ne "") {
    $m = [regex]::Match($text, "(?m)^.*($stopon).*$")
    if ($m.Success) { $stopped = "stopon"; Write-Output ("STOP after {0:N0} s: {1}" -f ((Get-Date) - $t0).TotalSeconds, $m.Value.Trim()); break }
  }
}
$h = [W]::FindByPid([uint32]$p.Id)
$sb = New-Object System.Text.StringBuilder 256
[W]::GetWindowText($h, $sb, 256) | Out-Null
Write-Output ("window: " + $sb.ToString())
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
if ($stopped -eq "crash") { $out = $out.Replace('.png', '-crash.png'); $shots = 1 }
Grab $out
for ($i = 1; $i -lt $shots; $i++) {
  Start-Sleep -Seconds $gap
  Grab ($out.Replace('.png', "_$i.png"))
}
# Close the window first so ares writes the cartridge save (<rom>.ram) on exit.
$p.CloseMainWindow() | Out-Null
if (-not $p.WaitForExit(8000)) { Stop-Process -Id $p.Id -Force }
if ($stopped -eq "crash") { exit 3 }
