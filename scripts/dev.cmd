@echo off
rem Development run on Windows. See scripts\dev.ps1.
rem   scripts\dev.cmd [app flags]      for example: scripts\dev.cmd --onboarding --dark
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" %*
