@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0..\.."

echo [INFO] Signing the extension version saved by update_extension_version.bat.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\sign_extension.ps1" %*
set "EXIT_CODE=%ERRORLEVEL%"

if not "%EXIT_CODE%"=="0" (
    echo.
    echo [ERROR] Extension signing or deployment failed!
    if not defined CI if not defined NO_PAUSE pause
    exit /b %EXIT_CODE%
)

echo.
if not defined CI if not defined NO_PAUSE pause
exit /b 0
