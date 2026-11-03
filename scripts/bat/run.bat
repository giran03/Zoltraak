@echo off
setlocal enabledelayedexpansion

:: Zoltraak App Launcher
cd /d "%~dp0..\.."

if "%1"=="web" goto run_web
if "%1"=="frontend" goto run_web
if "%1"=="engine" goto run_engine
if "%1"=="serve" goto run_engine

:run_tauri
echo ===================================
echo   Starting Zoltraak Desktop App (Tauri)
echo ===================================
cd app
if not exist "node_modules" (
    echo node_modules not found. Installing app dependencies...
    call npm install
)
call npm run app:dev
goto end

:run_web
echo ===================================
echo   Starting Zoltraak Web Mode (Engine + Frontend)
echo ===================================
netstat -ano | findstr ":1450 " >nul 2>&1
if errorlevel 1 (
    echo Starting background Zoltraak engine on port 1450...
    start "Zoltraak Engine" /B cargo run -p zoltraak-cli --manifest-path "%~dp0..\..\engine\Cargo.toml" -- serve
) else (
    echo Zoltraak engine is already active on port 1450.
)
cd app
if not exist "node_modules" (
    echo node_modules not found. Installing app dependencies...
    call npm install
)
call npm run dev
goto end

:run_engine
echo ===================================
echo   Starting Zoltraak Headless Engine
echo ===================================
cd engine
call cargo run -p zoltraak-cli -- serve
goto end

:end
endlocal