# Measure a tako window on Windows in TRUE PHYSICAL pixels (#1063). ASCII only.
#
# Why this script exists
# ----------------------
# Windows virtualises coordinates for DPI-unaware processes but NOT the GDI screen
# capture. A hand-rolled helper that forgets to declare DPI awareness therefore reads
# GetWindowRect as (physical / scale) while CopyFromScreen returns physical pixels.
# Mixing the two makes a perfectly healthy window look like the layout overflows it by
# exactly the display scale -- that is the whole of issue #1063 (measured: the tool
# reported 1550x830 for a window whose real client area was 1920x1020 at 125%, and the
# "screenshot" was a 1550x830 CROP of it, so the right/bottom fifth of the UI was
# simply missing from the image).
#
# So: always measure with this script, or with something that declares PerMonitorV2 and
# verifies that the declaration took effect. It refuses to run if it cannot.
#
# PowerShell 5.1 reads a BOM-less .ps1 as the system code page, so keep this file ASCII.
#
# Usage (run it in the interactive session -- see .agent/plans/2026-08-windows-main-merge-wip.md):
#   powershell -NoProfile -ExecutionPolicy Bypass -File measure-window.ps1 `
#       -TakoPid 1234 [-Png out.png] [-Maximize] [-ClickX 165 -ClickY 27]
#
# Output is a set of "key = value" lines on stdout plus an optional PNG.
param(
  [Parameter(Mandatory = $true)][int]$TakoPid,
  [string]$Png = "",
  [switch]$Maximize,
  [int]$ClickX = -1,
  [int]$ClickY = -1
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public class TakoDpiProbe {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
  [DllImport("user32.dll")] public static extern IntPtr GetWindowDpiAwarenessContext(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetAwarenessFromDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m, int t, out uint x, out uint y);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = (HANDLE)-4
[void][TakoDpiProbe]::SetProcessDpiAwarenessContext([IntPtr](-4))
$selfAwareness = [TakoDpiProbe]::GetAwarenessFromDpiAwarenessContext([TakoDpiProbe]::GetThreadDpiAwarenessContext())
Write-Output ("probe_awareness = " + $selfAwareness + " (0=unaware 1=system 2=per-monitor)")
if ($selfAwareness -ne 2) {
  Write-Error ("This probe could not become per-monitor DPI aware (got " + $selfAwareness + "). " +
    "Every number it prints would be virtualised while screenshots stay physical -- refusing to " +
    "produce misleading evidence. See issue #1063.")
  exit 2
}
Write-Output ("screen = " + [TakoDpiProbe]::GetSystemMetrics(0) + " x " + [TakoDpiProbe]::GetSystemMetrics(1))

$script:found = @()
$cb = [TakoDpiProbe+EnumProc]{ param($h, $p)
  $other = [uint32]0
  [void][TakoDpiProbe]::GetWindowThreadProcessId($h, [ref]$other)
  if ($other -eq [uint32]$TakoPid -and [TakoDpiProbe]::IsWindowVisible($h)) {
    $r = New-Object TakoDpiProbe+RECT
    [void][TakoDpiProbe]::GetWindowRect($h, [ref]$r)
    $sb = New-Object System.Text.StringBuilder 512
    [void][TakoDpiProbe]::GetWindowText($h, $sb, 512)
    $script:found += [pscustomobject]@{
      H = $h; Title = $sb.ToString(); L = $r.Left; T = $r.Top
      W = ($r.Right - $r.Left); Ht = ($r.Bottom - $r.Top)
    }
  }
  return $true
}
[void][TakoDpiProbe]::EnumWindows($cb, [IntPtr]::Zero)
foreach ($f in $script:found) {
  Write-Output ("candidate = hwnd " + $f.H + " [" + $f.Title + "] " + $f.W + "x" + $f.Ht + " at " + $f.L + "," + $f.T)
}
$w = $script:found | Where-Object { $_.W -gt 300 } | Select-Object -First 1
if (-not $w) { Write-Error ("no visible window larger than 300px for pid " + $TakoPid); exit 1 }
if ($Maximize) { [void][TakoDpiProbe]::ShowWindow($w.H, 3); Start-Sleep -Milliseconds 1500 }
[void][TakoDpiProbe]::SetForegroundWindow($w.H)
Start-Sleep -Milliseconds 1200

$wr = New-Object TakoDpiProbe+RECT; [void][TakoDpiProbe]::GetWindowRect($w.H, [ref]$wr)
$cr = New-Object TakoDpiProbe+RECT; [void][TakoDpiProbe]::GetClientRect($w.H, [ref]$cr)
$dpi = [TakoDpiProbe]::GetDpiForWindow($w.H)
$scale = $dpi / 96.0
$mon = [TakoDpiProbe]::MonitorFromWindow($w.H, 2)
$mdx = 0; $mdy = 0
[void][TakoDpiProbe]::GetDpiForMonitor($mon, 0, [ref]$mdx, [ref]$mdy)
Write-Output ("target_hwnd = " + $w.H)
Write-Output ("target_awareness = " + [TakoDpiProbe]::GetAwarenessFromDpiAwarenessContext([TakoDpiProbe]::GetWindowDpiAwarenessContext($w.H)) + " (tako must be 2)")
Write-Output ("window_rect_physical = " + ($wr.Right - $wr.Left) + " x " + ($wr.Bottom - $wr.Top) + " at " + $wr.Left + "," + $wr.Top)
Write-Output ("client_rect_physical = " + ($cr.Right - $cr.Left) + " x " + ($cr.Bottom - $cr.Top))
Write-Output ("dpi_for_window = " + $dpi + "  scale = " + $scale)
Write-Output ("dpi_for_monitor = " + $mdx + " x " + $mdy)
Write-Output ("client_logical = " + [math]::Round(($cr.Right - $cr.Left) / $scale, 2) + " x " + [math]::Round(($cr.Bottom - $cr.Top) / $scale, 2))

if ($ClickX -ge 0 -and $ClickY -ge 0) {
  $sx = $wr.Left + $ClickX
  $sy = $wr.Top + $ClickY
  Write-Output ("click_window_relative_physical = " + $ClickX + "," + $ClickY + " -> screen " + $sx + "," + $sy)
  [void][TakoDpiProbe]::SetCursorPos($sx, $sy)
  Start-Sleep -Milliseconds 300
  [TakoDpiProbe]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 90
  [TakoDpiProbe]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 1200
}

if ($Png -ne "") {
  $bmp = New-Object System.Drawing.Bitmap ($wr.Right - $wr.Left), ($wr.Bottom - $wr.Top)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($wr.Left, $wr.Top, 0, 0, $bmp.Size)
  $bmp.Save($Png, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Write-Output ("saved_png = " + $Png + " (" + ($wr.Right - $wr.Left) + "x" + ($wr.Bottom - $wr.Top) + ")")
}
