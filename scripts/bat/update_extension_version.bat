@echo off
setlocal

:: Zoltraak Extension Version Manager
cd /d "%~dp0..\.."
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\update_extension_version.ps1" %*
set "EXIT_CODE=%ERRORLEVEL%"

if not "%EXIT_CODE%"=="0" (
    echo.
    echo [ERROR] Extension version update did not complete successfully.
)

if not defined CI if not defined NO_PAUSE pause
exit /b %EXIT_CODE%
