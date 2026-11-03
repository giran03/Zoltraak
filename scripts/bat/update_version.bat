@echo off
setlocal enabledelayedexpansion

:: Zoltraak Version Updater Batch Script
cd /d "%~dp0..\.."

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\update_version.ps1" %*

if %ERRORLEVEL% neq 0 (
    echo.
    echo [ERROR] Version update did not complete successfully.
    if not defined CI if not defined NO_PAUSE pause
    exit /b %ERRORLEVEL%
)

if not defined CI if not defined NO_PAUSE pause
exit /b 0
