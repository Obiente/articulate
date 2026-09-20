@echo off
cd /d "%~dp0"
if not exist "target\release\transcribe-local.exe" (
  echo Build the app first: powershell -File scripts\build-windows.ps1
  pause
  exit /b 1
)
start "" "target\release\transcribe-local.exe"
