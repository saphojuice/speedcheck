@echo off
REM SAPHOJUICE speed check, double-click version for Windows.
REM
REM Same logic as the one-liner. It runs the canonical script, scripts/check.ps1 in
REM github.com/saphojuice/speedcheck, so there is only one implementation to audit:
REM download the probe, verify its SHA-256 against the published release, run it, delete it.
REM
REM Nothing is installed. Because you downloaded this .bat file, Windows may warn that it
REM came from the internet. Pasting the one-liner into PowerShell avoids that warning entirely:
REM     irm https://saphojuice.com/check.ps1 ^| iex
setlocal
echo.
echo   SAPHOJUICE speed check
echo   Nothing installs. The probe is deleted when it finishes.
echo.
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://saphojuice.com/check.ps1 | iex"
echo.
echo   Press any key to close this window.
pause >nul
endlocal
