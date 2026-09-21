@echo off
setlocal
cd /d "%~dp0"
set "ARTICULATE_TARGET=target"
if defined CARGO_TARGET_DIR set "ARTICULATE_TARGET=%CARGO_TARGET_DIR%"
set "ARTICULATE_PROFILE=release"
if /I "%~1"=="--debug" set "ARTICULATE_PROFILE=debug"
if not exist "%ARTICULATE_TARGET%\%ARTICULATE_PROFILE%\articulate.exe" (
  echo Build the app first: powershell -File scripts\build-windows.ps1
  pause
  exit /b 1
)
start "" "%ARTICULATE_TARGET%\%ARTICULATE_PROFILE%\articulate.exe"
