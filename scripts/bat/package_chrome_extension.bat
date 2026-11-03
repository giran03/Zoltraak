@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0..\.."

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\package_chrome_extension.ps1" %*

if %ERRORLEVEL% neq 0 (
    echo.
    echo [ERROR] Packaging failed!
    pause
    exit /b %ERRORLEVEL%
)

echo.
pause
