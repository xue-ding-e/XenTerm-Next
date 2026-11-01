@echo off
setlocal
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\update-local.ps1" -WorkspaceRoot "%~dp0.." -Pause %*
exit /b %errorlevel%
