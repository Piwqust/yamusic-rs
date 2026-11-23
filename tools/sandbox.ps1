# Проверочный запуск yamusic-rs рядом с рабочим экземпляром: своя папка настроек, окно
# невидимо и не берёт фокус (YAMUSIC_RS_SANDBOX, см. config::sandbox), звук выключен.
# Токен и устройство берутся из настоящей папки настроек, сама она не меняется.
#   tools\sandbox.ps1 -Launch [-Exe target\release\yamusic-rs.exe] [-Mode smooth|lite]
#   tools\sandbox.ps1 -Resize 1900,1150              размер окна в пикселях
#   tools\sandbox.ps1 -Cmd "move 500 300","click 500 300","wheel 500 300 -60","leave"
#                                                     мышь в логических пикселях окна
#   tools\sandbox.ps1 -Shot out.png [-Crop x,y,w,h]   снимок окна через PrintWindow
#   tools\sandbox.ps1 -Info                           размер окна, время процессора, память
#   tools\sandbox.ps1 -Stop
param(
    [switch]$Launch, [string]$Exe, [int]$Width = 0, [int]$Height = 0,
    [string]$Shot, [int[]]$Click, [int[]]$Resize, [int[]]$Hover, [string[]]$Cmd, [int]$Wait = 0, [int[]]$Crop, [switch]$Stop, [switch]$Info, [string]$Mode = "smooth"
)
$ErrorActionPreference = "Stop"
$root = Join-Path $env:TEMP "yamusic-rs-sandbox"
New-Item -ItemType Directory -Force $root | Out-Null
if (-not $Exe) { $Exe = Join-Path (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)) "target\release\yamusic-rs.exe" }
$home2 = Join-Path $root "sandbox-appdata"
$pidFile = Join-Path $root "sandbox.pid"

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class SbWin {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lp);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder s, int n);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static IntPtr Find(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); RECT r; GetWindowRect(h, out r);
            if (p == pid && IsWindowVisible(h) && r.R - r.L > 300) { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
}
"@
Add-Type -AssemblyName System.Drawing
# Настоящие пиксели, а не виртуальные: иначе окно на мониторе с масштабом снимается обрезанным.
[SbWin]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null

function Get-SandboxPid {
    if (-not (Test-Path $pidFile)) { throw "песочница не запущена" }
    [uint32](Get-Content $pidFile)
}

if ($Launch) {
    if (Test-Path $pidFile) { try { Stop-Process -Id (Get-Content $pidFile) -Force -ErrorAction Stop } catch {} }
    $cfgDir = Join-Path $home2 "yamusic-rs"
    New-Item -ItemType Directory -Force $cfgDir | Out-Null
    $src = Join-Path $env:APPDATA "yamusic-rs"
    foreach ($f in "device", "wave.json") { if (Test-Path "$src\$f") { Copy-Item "$src\$f" $cfgDir -Force } }
    $cfg = Get-Content "$src\config.json" -Raw -Encoding UTF8 | ConvertFrom-Json
    $cfg.volume = 0.0
    $cfg.discord_enabled = $false
    $cfg.check_updates = $false
    $cfg.mode = $Mode
    $cfg.window_pos = $null
    if ($Width -gt 0) { $cfg.window_size = @($Width, $Height) } else { $cfg.window_size = $null }
    $cfg.window_maximized = $false
    [IO.File]::WriteAllText("$cfgDir\config.json", ($cfg | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
    Get-Process | Where-Object { $_.Path -like "$root
un-*.exe" } | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    Get-ChildItem "$root
un-*.exe" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    $copy = Join-Path $root ("run-" + (Get-Date -Format "HHmmss") + ".exe")
    Copy-Item $Exe $copy -Force
    $Exe = $copy
    $oldApp = $env:APPDATA
    $env:APPDATA = $home2
    $env:YAMUSIC_RS_SANDBOX = "1"
    $p = Start-Process -FilePath $Exe -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $root "sandbox.err") -RedirectStandardOutput (Join-Path $root "sandbox.out")
    $env:APPDATA = $oldApp
    Remove-Item Env:\YAMUSIC_RS_SANDBOX
    Set-Content $pidFile $p.Id
    "pid $($p.Id)"
    return
}

if ($Stop) {
    $id = Get-SandboxPid
    Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    Remove-Item $pidFile -Force
    "stopped $id"
    return
}

$id = Get-SandboxPid
$hwnd = [SbWin]::Find($id)
if ($hwnd -eq [IntPtr]::Zero) { throw "окно песочницы не найдено" }

if ($Info) {
    $r = New-Object SbWin+RECT; [SbWin]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    $c = New-Object SbWin+RECT; [SbWin]::GetClientRect($hwnd, [ref]$c) | Out-Null
    $proc = Get-Process -Id $id
    "hwnd $hwnd pos $($r.L),$($r.T) size $($r.R - $r.L)x$($r.B - $r.T) client $($c.R)x$($c.B) cpu $([math]::Round($proc.TotalProcessorTime.TotalSeconds, 2))s mem $([math]::Round($proc.WorkingSet64 / 1MB, 1))MB"
}

if ($Cmd) {
    [IO.File]::WriteAllText((Join-Path $home2 "yamusic-rs\sandbox.cmd"), ($Cmd -join "`n"))
    "cmd $($Cmd -join '; ')"
}
if ($Wait -gt 0) { Start-Sleep -Milliseconds $Wait }

if ($Resize) {
    [SbWin]::SetWindowPos($hwnd, [IntPtr]::Zero, 0, 0, $Resize[0], $Resize[1], 0x0016) | Out-Null   # NOMOVE|NOZORDER|NOACTIVATE
    "resize $($Resize -join 'x')"
}

if ($Hover) {
    $lp = [IntPtr](($Hover[1] -shl 16) -bor ($Hover[0] -band 0xffff))
    [SbWin]::PostMessage($hwnd, 0x0200, [IntPtr]::Zero, $lp) | Out-Null
    "hover $($Hover -join ',')"
}

if ($Click) {
    $lp = [IntPtr](($Click[1] -shl 16) -bor ($Click[0] -band 0xffff))
    [SbWin]::PostMessage($hwnd, 0x0200, [IntPtr]::Zero, $lp) | Out-Null   # WM_MOUSEMOVE
    Start-Sleep -Milliseconds 60
    [SbWin]::PostMessage($hwnd, 0x0201, [IntPtr]1, $lp) | Out-Null        # WM_LBUTTONDOWN
    Start-Sleep -Milliseconds 60
    [SbWin]::PostMessage($hwnd, 0x0202, [IntPtr]::Zero, $lp) | Out-Null   # WM_LBUTTONUP
    "click $($Click -join ',')"
}

if ($Shot) {
    $r = New-Object SbWin+RECT; [SbWin]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    $w = $r.R - $r.L; $h = $r.B - $r.T
    $bmp = New-Object Drawing.Bitmap $w, $h
    $g = [Drawing.Graphics]::FromImage($bmp)
    $dc = $g.GetHdc()
    [SbWin]::PrintWindow($hwnd, $dc, 2) | Out-Null    # PW_RENDERFULLCONTENT
    $g.ReleaseHdc($dc); $g.Dispose()
    if ($Crop) { $c = $bmp.Clone((New-Object Drawing.Rectangle $Crop[0], $Crop[1], $Crop[2], $Crop[3]), $bmp.PixelFormat); $bmp.Dispose(); $bmp = $c }
    $bmp.Save($Shot, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    "shot $Shot ${w}x${h}"
}
