# enable-autostart.ps1 — ainput 开机自启受管入口（用户级，无需管理员，幂等可重跑）：
#   1) HKCU Run 键直指 exe 本体（不经 run-ainput.bat，无黑窗一闪；
#      exe 按自身路径定位模型/配置，不依赖工作目录）
#   2) StartupApproved 首字节置 02（任务管理器“启动应用”里保持启用；
#      03=禁用是此前自启失效的直接原因）
#   3) Startup 文件夹放快捷方式（双保险：任一存活即可自启；
#      单实例锁保证不会起两个）
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts\enable-autostart.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'ainput.exe'
if (-not (Test-Path $exe)) { throw "找不到 $exe（先跑 cargo build --release + deploy）" }

# 1) Run 键
New-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' `
  -Name 'ainput' -Value ('"' + $exe + '"') -PropertyType String -Force | Out-Null

# 2) Approved=启用（保留原有字节，只翻首字节 03->02）
$approvedPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$cur = (Get-ItemProperty -Path $approvedPath -Name 'ainput' -ErrorAction SilentlyContinue).ainput
if ($cur) {
  $cur[0] = 2
  Set-ItemProperty -Path $approvedPath -Name 'ainput' -Value ([byte[]]$cur)
} else {
  New-ItemProperty -Path $approvedPath -Name 'ainput' `
    -Value ([byte[]](2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) -PropertyType Binary -Force | Out-Null
}

# 3) Startup 快捷方式
$ws = New-Object -ComObject WScript.Shell
$lnkPath = Join-Path ([Environment]::GetFolderPath('Startup')) 'ainput.lnk'
$lnk = $ws.CreateShortcut($lnkPath)
$lnk.TargetPath = $exe
$lnk.WorkingDirectory = $root
$lnk.IconLocation = "$exe,0"
$lnk.Description = 'ainput voice input (autostart)'
$lnk.Save()

Write-Host '自启已启用：Run 键 + Startup 快捷方式'
Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ainput' |
  Select-Object -ExpandProperty 'ainput'
Write-Host "快捷方式：$lnkPath"
