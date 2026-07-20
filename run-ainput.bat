@echo off
setlocal
cd /d "%~dp0"
if exist "%~dp0ainput.exe" (
  start "" "%~dp0ainput.exe"
) else (
  start "" "%~dp0target\release\ainput.exe"
)
