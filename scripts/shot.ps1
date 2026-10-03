# Screenshot of the largest visible window of a process, for design review on Windows.
# The Windows counterpart of scripts/shot.swift.
#
#   powershell -ExecutionPolicy Bypass -File scripts/shot.ps1 sayso $env:TEMP\shot.png
#   powershell -ExecutionPolicy Bypass -File scripts/shot.ps1 sayso $env:TEMP\screen.png -Screen   # the whole primary display
param(
    [Parameter(Mandatory = $true)][string]$Process,
    [Parameter(Mandatory = $true)][string]$Out,
    [switch]$Screen
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SaysoShot {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int attr, out RECT rect, int size);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
[SaysoShot]::SetProcessDPIAware() | Out-Null

if ($Screen) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
} else {
    $ids = @(Get-Process -Name $Process -ErrorAction Stop | ForEach-Object { [uint32]$_.Id })
    $best = $null
    $bestArea = 0
    $cb = [SaysoShot+EnumProc]{
        param($hwnd, $l)
        $procId = [uint32]0
        [SaysoShot]::GetWindowThreadProcessId($hwnd, [ref]$procId) | Out-Null
        if ($ids -contains $procId -and [SaysoShot]::IsWindowVisible($hwnd)) {
            $r = New-Object SaysoShot+RECT
            # DWMWA_EXTENDED_FRAME_BOUNDS: the visible frame, without the invisible resize border.
            [SaysoShot]::DwmGetWindowAttribute($hwnd, 9, [ref]$r, 16) | Out-Null
            $area = ($r.Right - $r.Left) * ($r.Bottom - $r.Top)
            if ($area -gt $script:bestArea) { $script:bestArea = $area; $script:best = $r }
        }
        return $true
    }
    [SaysoShot]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
    if ($null -eq $best) { throw "no visible window of $Process" }
    $bounds = New-Object System.Drawing.Rectangle $best.Left, $best.Top, ($best.Right - $best.Left), ($best.Bottom - $best.Top)
}
$bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "$Out ($($bounds.Width)x$($bounds.Height))"
