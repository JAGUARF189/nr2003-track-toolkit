# Launch the toolkit GUI with test settings, optionally click points, and screenshot it.
# usage: gui_shot.ps1 -Out shot.png [-Clicks "x,y;x,y"] [-Keep] [-AppArgs "file.ptf"]
param([string]$Out, [string]$Clicks = "", [switch]$Keep, [int]$Wait = 4, [string]$AppArgs = "")
$e = "C:\Papyrus\TrackToolkit\_test\e2e"
New-Item -ItemType Directory -Force "$e\appdata\TrackToolkit" | Out-Null
Copy-Item "$e\settings.json" "$e\appdata\TrackToolkit\settings.json" -Force
Add-Type @'
using System; using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(int f, int x, int y, int d, int e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, int f, int e);
  public struct RECT { public int L, T, R, B; }
  public struct POINT { public int X, Y; }
}
'@
[W]::SetProcessDPIAware() | Out-Null
$psi = New-Object System.Diagnostics.ProcessStartInfo "C:\Papyrus\TrackToolkit\target\release\track-toolkit.exe"
$psi.Arguments = $AppArgs
$psi.UseShellExecute = $false
$psi.EnvironmentVariables["APPDATA"] = "$e\appdata"
$p = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds $Wait
$p.Refresh(); $h = $p.MainWindowHandle
[W]::SetForegroundWindow($h) | Out-Null; Start-Sleep -Milliseconds 500
# Steps: "x,y" = click (client coordinates, physical pixels), "k:text" = type keys (SendKeys syntax),
# "w:ms" = wait, "s:file.png" = screenshot now, "d:x1,y1,x2,y2" = left-drag, "r:x1,y1,x2,y2" = right-drag,
# "c:x,y" = Ctrl+click.
$shell = New-Object -ComObject WScript.Shell
foreach ($c in ($Clicks -split ';' | Where-Object { $_ })) {
  # Never send input to anything but the toolkit (or its own file dialog, which it owns).
  if (-not $c.StartsWith('w:') -and -not $c.StartsWith('s:')) {
    $fgPid = [uint32]0; $null = [W]::GetWindowThreadProcessId([W]::GetForegroundWindow(), [ref]$fgPid)
    if ($fgPid -ne $p.Id) {
      $owner = (Get-Process -Id $fgPid -ErrorAction SilentlyContinue).ProcessName
      "ABORT before '$c': the window in front belongs to '$owner', not the toolkit. No input sent."
      Stop-Process -Id $p.Id -ErrorAction SilentlyContinue; exit 1
    }
  }
  if ($c.StartsWith('k:')) { $shell.SendKeys($c.Substring(2)); Start-Sleep -Milliseconds 800; continue }
  if ($c.StartsWith('w:')) { Start-Sleep -Milliseconds ([int]$c.Substring(2)); continue }
  if ($c.StartsWith('s:')) {
    $r0 = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r0) | Out-Null; Add-Type -AssemblyName System.Drawing
    $b0 = New-Object System.Drawing.Bitmap ($r0.R - $r0.L), ($r0.B - $r0.T)
    [System.Drawing.Graphics]::FromImage($b0).CopyFromScreen($r0.L, $r0.T, 0, 0, $b0.Size); $b0.Save($c.Substring(2)); continue
  }
  if ($c.StartsWith('d:') -or $c.StartsWith('r:')) {
    $v = $c.Substring(2) -split ','; $down = 2; $up = 4; if ($c.StartsWith('r:')) { $down = 8; $up = 16 }
    $a = New-Object W+POINT; $a.X = [int]$v[0]; $a.Y = [int]$v[1]; [W]::ClientToScreen($h, [ref]$a) | Out-Null
    $b = New-Object W+POINT; $b.X = [int]$v[2]; $b.Y = [int]$v[3]; [W]::ClientToScreen($h, [ref]$b) | Out-Null
    [W]::SetCursorPos($a.X, $a.Y) | Out-Null; Start-Sleep -Milliseconds 150
    [W]::mouse_event($down, 0, 0, 0, 0); Start-Sleep -Milliseconds 100
    for ($i = 1; $i -le 20; $i++) {
      [W]::SetCursorPos([int]($a.X + ($b.X - $a.X) * $i / 20), [int]($a.Y + ($b.Y - $a.Y) * $i / 20)) | Out-Null
      Start-Sleep -Milliseconds 30
    }
    Start-Sleep -Milliseconds 100; [W]::mouse_event($up, 0, 0, 0, 0); Start-Sleep -Seconds 1; continue
  }
  $ctrl = $c.StartsWith('c:'); if ($ctrl) { $c = $c.Substring(2); [W]::keybd_event(0x11, 0, 0, 0); Start-Sleep -Milliseconds 80 }
  $xy = $c -split ','; $pt = New-Object W+POINT; $pt.X = [int]$xy[0]; $pt.Y = [int]$xy[1]
  [W]::ClientToScreen($h, [ref]$pt) | Out-Null
  [W]::SetCursorPos($pt.X, $pt.Y) | Out-Null; Start-Sleep -Milliseconds 150
  [W]::mouse_event(2, 0, 0, 0, 0); Start-Sleep -Milliseconds 60; [W]::mouse_event(4, 0, 0, 0, 0)
  if ($ctrl) { Start-Sleep -Milliseconds 80; [W]::keybd_event(0x11, 0, 2, 0) }
  Start-Sleep -Seconds 2
}
$r = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r) | Out-Null
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $bmp.Save($Out)
"pid=$($p.Id) alive=$(-not $p.HasExited) size=$($bmp.Width)x$($bmp.Height)"
if (-not $Keep) { Stop-Process -Id $p.Id -ErrorAction SilentlyContinue }
