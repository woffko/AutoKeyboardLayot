@echo off
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0run-notepad-uia-probe.ps1" %*
exit /b %ERRORLEVEL%
