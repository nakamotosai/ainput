@echo off
set PATH=%USERPROFILE%\.cargo\bin;%PATH%
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -File "F:\projects\ainput\scripts\make-portable.ps1"