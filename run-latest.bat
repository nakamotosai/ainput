@echo off
setlocal

cd /d "%~dp0"
set "AINPUT_ROOT=%~dp0"

echo [ainput] Stopping old process...
taskkill /IM ainput-desktop.exe /F >nul 2>nul

where cargo >nul 2>nul
if errorlevel 1 (
    echo [ainput] cargo not found. Please install Rust and make sure cargo is in PATH.
    pause
    exit /b 1
)

echo [ainput] Building latest version...
cargo build -p ainput-desktop
if errorlevel 1 (
    echo [ainput] Build failed.
    pause
    exit /b 1
)

if not exist "target\debug\ainput-desktop.exe" (
    echo [ainput] target\debug\ainput-desktop.exe not found.
    pause
    exit /b 1
)

echo [ainput] Launching latest version...
start "" "%~dp0target\debug\ainput-desktop.exe"
exit /b 0
