# deploy.ps1 — 一键部署新版 ainput：
#   1) 给正在运行的托盘窗口发「请退出」消息（与右键退出同一路径，体面收尾）
#   2) 等老进程死透（需要等 exe 文件锁释放才能覆盖）
#   3) 拷贝新 exe 并拉起
# 老版本没有这条消息处理时会自动退化成强杀兜底。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts\deploy.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe  = Join-Path $root 'ainput.exe'
$new  = Join-Path $root 'target\release\ainput.exe'

if (-not (Test-Path $new)) { throw "新 exe 不存在：$new（先跑 cargo build --release）" }

# 2026-09-02 踩坑：本机 PowerShell 的 FindWindowW P/Invoke 始终返回 0
# （ctypes/python 与 EnumWindows 都能命中同样类名，原因未查明），
# 因此找托盘窗口改用 EnumWindows 遍历 + GetClassNameW 匹配，实测有效。
$win32 = @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class AinputWin32 {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    public static IntPtr FindByClass(string want) {
        IntPtr found = IntPtr.Zero;
        EnumWindowsProc cb = (h, l) => {
            var sb = new StringBuilder(256);
            GetClassNameW(h, sb, sb.Capacity);
            if (sb.ToString() == want) { found = h; return false; }
            return true;
        };
        EnumWindows(cb, IntPtr.Zero);
        System.GC.KeepAlive(cb);
        return found;
    }
}
'@
Add-Type -TypeDefinition $win32

# WM_APP(0x8000) + 47 = TRAY_REMOTE_QUIT_MSG（保持与 tray.rs 中的常量一致）
$TRAY_REMOTE_QUIT = 0x8000 + 47

$proc = Get-Process ainput -ErrorAction SilentlyContinue
if ($proc) {
    $hwnd = [AinputWin32]::FindByClass('ainput_tray_window')
    if ($hwnd -ne [System.IntPtr]::Zero) {
        [void][AinputWin32]::PostMessageW($hwnd, $TRAY_REMOTE_QUIT, [System.IntPtr]::Zero, [System.IntPtr]::Zero)
        Write-Host '已向托盘发送退出请求，等待收尾…'
    } else {
        Write-Host '没找到托盘窗口，直接强杀兜底'
    }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Process ainput -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 300
    }
    if (Get-Process ainput -ErrorAction SilentlyContinue) {
        Write-Host '15 秒内未退出（老版本无远程退出支持），强杀'
        Stop-Process -Name ainput -Force
        Start-Sleep -Seconds 1
    }
    Write-Host '旧进程已退出'
} else {
    Write-Host 'ainput 未在运行，直接换装'
}

Copy-Item $new $exe -Force
Start-Process $exe -WorkingDirectory $root
Start-Sleep -Seconds 2
$running = Get-Process ainput -ErrorAction SilentlyContinue
if (-not $running) { throw '新进程拉起失败' }
Write-Host ("部署完成：PID {0}，exe 时间戳 {1}" -f $running.Id, (Get-Item $exe).LastWriteTime)
