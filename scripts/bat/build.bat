@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0..\.."
call "%~dp0build_app.bat" %*
exit /b %ERRORLEVEL%
