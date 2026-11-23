# Start the toolkit with test settings and capture its window with PrintWindow.
# Sends NO input and never needs focus, so it's safe while you use the PC (or play).
# usage: win_capture.ps1 -Out shot.png [-AppArgs "file.ptf --3d"] [-Wait 5]
param([string]$Out, [string]$AppArgs = "", [int]$Wait = 5)
$e = "C:\Papyrus\TrackToolkit\_test\e2e"
New-Item -ItemType Directory -Force "$e\appdata\TrackToolkit" | Out-Null
Copy-Item "$e\settings.json" "$e\appdata\TrackToolkit\settings.json" -Force
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public class PW {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public struct RECT { public int L, T, R, B; }
}
'@
[PW]::SetProcessDPIAware() | Out-Null
$psi = New-Object System.Diagnostics.ProcessStartInfo "C:\Papyrus\TrackToolkit\target\release\track-toolkit.exe"
$psi.Arguments = $AppArgs
$psi.UseShellExecute = $false
$psi.EnvironmentVariables["APPDATA"] = "$e\appdata"
$p = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds $Wait
$p.Refresh(); $h = $p.MainWindowHandle
$r = New-Object PW+RECT; [PW]::GetWindowRect($h, [ref]$r) | Out-Null
$bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
$g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
[PW]::PrintWindow($h, $hdc, 2) | Out-Null   # 2 = PW_RENDERFULLCONTENT (includes OpenGL content)
$g.ReleaseHdc($hdc); $bmp.Save($Out)
Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
"captured $($r.R - $r.L)x$($r.B - $r.T)"
