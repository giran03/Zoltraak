@echo off
setlocal enabledelayedexpansion

:: Ensure script runs relative to the project root
cd /d "%~dp0..\.."

echo ====================================================
echo   OpenDM / Zoltraak - Desktop App Builder (Tauri)
echo ====================================================
echo.

:: 1. Check for Node.js and npm
where.exe node >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo [ERROR] Node.js is not installed or not found in PATH.
    echo Please install Node.js 20+ from https://nodejs.org/
    goto error_exit
)

where.exe npm >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo [ERROR] npm is not installed or not found in PATH.
    goto error_exit
)

:: 2. Check for Rust and Cargo
where.exe cargo >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo [ERROR] Rust / Cargo is not installed or not found in PATH.
    echo Please install Rust 1.85+ from https://rustup.rs/
    goto error_exit
)

where.exe rustc >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo [ERROR] rustc is not installed or not found in PATH.
    goto error_exit
)

for /f "tokens=*" %%i in ('node -v') do set NODE_VER=%%i
for /f "tokens=*" %%i in ('cargo -V') do set CARGO_VER=%%i
echo [INFO] Found Node.js: %NODE_VER%
echo [INFO] Found Cargo:   %CARGO_VER%
echo.

:: Check for help flags
if /i "%~1"=="help" goto show_help
if /i "%~1"=="--help" goto show_help
if /i "%~1"=="-h" goto show_help

:: Subcommand routing (targets that do NOT build Tauri desktop installer)
if /i "%~1"=="web" goto build_web
if /i "%~1"=="frontend" goto build_web
if /i "%~1"=="engine" goto build_engine
if /i "%~1"=="cli" goto build_engine

:: Check if this is 'all' build
set "IS_BUILD_ALL=0"
set "PASSTHROUGH_ARGS="
if /i "%~1"=="all" (
    set "IS_BUILD_ALL=1"
    for /f "tokens=1,* delims= " %%a in ("%*") do set "PASSTHROUGH_ARGS=%%b"
) else (
    set "PASSTHROUGH_ARGS=%*"
)

:: Check if bundle selection or --no-bundle was explicitly provided in command line
set "HAS_BUNDLE_ARG=0"
echo(%* | findstr /i /c:"--bundles" /c:"-b " /c:"--no-bundle" >nul 2>&1
if not errorlevel 1 set "HAS_BUNDLE_ARG=1"

set "BUNDLE_FLAG=--bundles nsis"
set "BUNDLE_DESC=NSIS (.exe installer)"

if "!HAS_BUNDLE_ARG!"=="1" (
    set "BUNDLE_FLAG="
    set "BUNDLE_DESC=Command-line selection"
    goto skip_bundle_selection
)
if defined CI goto skip_bundle_selection

echo ====================================================
echo   Installer Bundle Target Confirmation
echo ====================================================
echo Select which installer/package to build:
echo   [1] NSIS (.exe installer) [Default]
echo   [2] MSI / MSIX (.msi package)
echo   [3] Both (NSIS + MSI)
echo   [q] Cancel build
echo.

:ask_bundle
set "BUNDLE_CHOICE="
set /p "BUNDLE_CHOICE=Select target [1, 2, 3, or q] (default: 1): "
if not defined BUNDLE_CHOICE set "BUNDLE_CHOICE=1"
set "BUNDLE_CHOICE=!BUNDLE_CHOICE:"=!"
set "BUNDLE_CHOICE=!BUNDLE_CHOICE: =!"

if /i "!BUNDLE_CHOICE!"=="1" goto set_nsis
if /i "!BUNDLE_CHOICE!"=="nsis" goto set_nsis
if /i "!BUNDLE_CHOICE!"=="exe" goto set_nsis

if /i "!BUNDLE_CHOICE!"=="2" goto set_msi
if /i "!BUNDLE_CHOICE!"=="msi" goto set_msi
if /i "!BUNDLE_CHOICE!"=="msix" goto set_msi

if /i "!BUNDLE_CHOICE!"=="3" goto set_both
if /i "!BUNDLE_CHOICE!"=="both" goto set_both
if /i "!BUNDLE_CHOICE!"=="all" goto set_both

if /i "!BUNDLE_CHOICE!"=="q" goto cancel_build
if /i "!BUNDLE_CHOICE!"=="quit" goto cancel_build
if /i "!BUNDLE_CHOICE!"=="cancel" goto cancel_build

echo [WARN] Invalid choice "!BUNDLE_CHOICE!". Please enter 1, 2, 3, or 'q' to cancel.
echo.
goto ask_bundle

:set_nsis
set "BUNDLE_FLAG=--bundles nsis"
set "BUNDLE_DESC=NSIS (.exe installer)"
goto bundle_confirmed

:set_msi
set "BUNDLE_FLAG=--bundles msi"
set "BUNDLE_DESC=MSI / MSIX (.msi package)"
goto bundle_confirmed

:set_both
set "BUNDLE_FLAG=--bundles nsis,msi"
set "BUNDLE_DESC=Both (NSIS + MSI)"
goto bundle_confirmed

:bundle_confirmed
echo.
echo [INFO] Selected bundle: !BUNDLE_DESC!
echo.

:skip_bundle_selection

:: If 'all' was requested, build headless engine CLI first
if "!IS_BUILD_ALL!"=="1" (
    echo ====================================================
    echo   [1/2] Building Headless Engine CLI ^(Cargo^)
    echo ====================================================
    cd engine
    call cargo build --release -p zoltraak-cli
    set BUILD_ERR=!ERRORLEVEL!
    cd ..
    if !BUILD_ERR! neq 0 goto error_exit
    echo.
    echo [SUCCESS] Engine CLI built in engine\target\release\zoltraak-cli.exe
    echo.
    echo ====================================================
    echo   [2/2] Building Tauri Desktop App ^(!BUNDLE_DESC!^)
    echo ====================================================
)

:: 3. Verify / install frontend dependencies
if not exist "app\node_modules" (
    echo [INFO] app\node_modules not found. Installing frontend dependencies...
    cd app
    call npm install
    if !ERRORLEVEL! neq 0 (
        echo [ERROR] npm install failed.
        cd ..
        goto error_exit
    )
    cd ..
)

:: 4. Verify extension bundle resource
if not exist "app\src-tauri\resources\zoltraak-signed.xpi" (
    echo [WARN] app\src-tauri\resources\zoltraak-signed.xpi not found.
    if exist "scripts\sign_extension.ps1" (
        echo [INFO] Attempting to sign/package extension bundle...
        powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\sign_extension.ps1"
    )
)

:: Media tools are staged and validated by Tauri's beforeBuildCommand and build.rs.

:: 4c. Load Updater Signing Key (for signed release packages & latest.json)
if exist "%USERPROFILE%\.tauri\zoltraak.key" (
    echo [INFO] Detected update signing key: %USERPROFILE%\.tauri\zoltraak.key
    set TAURI_SIGNING_PRIVATE_KEY=%USERPROFILE%\.tauri\zoltraak.key
    set TAURI_SIGNING_PRIVATE_KEY_PATH=%USERPROFILE%\.tauri\zoltraak.key
    if not defined TAURI_SIGNING_PRIVATE_KEY_PASSWORD echo [WARN] Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD so the update signature can be created.
) else (
    echo [WARN] Update signing key not found. Build will proceed without update package signing.
    echo [INFO] Run powershell -File scripts\generate_update_keys.ps1 to enable auto-update packages.
)

:: 5. Build Tauri App
:: Validate the notes before baking the same CHANGELOG.json into the frontend.
call :audit_changelog
if !ERRORLEVEL! neq 0 goto error_exit
node scripts\changelog.mjs sync
if !ERRORLEVEL! neq 0 goto error_exit
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\read_release_notes.ps1" -ValidateOnly
if !ERRORLEVEL! neq 0 (
    echo [ERROR] Release notes validation failed. Update CHANGELOG.json for the current app version.
    goto error_exit
)

if "!IS_BUILD_ALL!"=="0" (
    echo.
    echo ====================================================
    echo   Building Tauri Desktop App ^(!BUNDLE_DESC!^)
    echo ====================================================
    echo.
)

cd app
call npx tauri build !BUNDLE_FLAG! !PASSTHROUGH_ARGS!
set BUILD_ERR=!ERRORLEVEL!
cd ..

if %BUILD_ERR% neq 0 (
    echo.
    echo [ERROR] Tauri app build failed with exit code %BUILD_ERR%.
    goto error_exit
)

:: 5b. Generate Updater Manifest (latest.json)
if exist "scripts\generate_latest_json.ps1" (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\generate_latest_json.ps1"
    if !ERRORLEVEL! neq 0 (
        echo [ERROR] Updater manifest generation failed.
        goto error_exit
    )
)

echo.
echo ====================================================
echo   Build Successful!
echo ====================================================
echo.
echo Artifacts located in:
echo   - Executable: app\src-tauri\target\release\
if exist "app\src-tauri\target\release\bundle" (
    echo   - Bundles / Installers: app\src-tauri\target\release\bundle\
    echo   - Updater Manifest:    app\src-tauri\target\release\bundle\latest.json
)
echo.
goto success_exit

:build_web
echo ====================================================
echo   Building Web Frontend (Vite)
echo ====================================================
cd app
if not exist "node_modules" (
    echo [INFO] Installing frontend dependencies...
    call npm install
    if !ERRORLEVEL! neq 0 (
        cd ..
        goto error_exit
    )
)
call npm run build
set BUILD_ERR=!ERRORLEVEL!
cd ..
if %BUILD_ERR% neq 0 goto error_exit
echo.
echo [SUCCESS] Frontend build completed in app\dist\
goto success_exit

:build_engine
echo ====================================================
echo   Building Headless Engine CLI (Cargo)
echo ====================================================
cd engine
call cargo build --release -p zoltraak-cli
set BUILD_ERR=!ERRORLEVEL!
cd ..
if %BUILD_ERR% neq 0 goto error_exit
echo.
echo [SUCCESS] Engine CLI built in engine\target\release\zoltraak-cli.exe
goto success_exit

:cancel_build
echo.
echo [INFO] Build cancelled by user. No build was performed.
if not defined CI if not defined NO_PAUSE pause
exit /b 0

:show_help
echo Usage:
echo   build_app.bat [target] [tauri options]
echo.
echo Targets:
echo   (no target)     Build the Tauri desktop app and NSIS installer (Default)
echo   all             Build both the engine CLI and the complete Tauri desktop app
echo   web, frontend   Build only the web frontend (TypeScript + Vite)
echo   engine, cli     Build only the headless engine CLI (zoltraak-cli)
echo   help, -h        Show this help message
echo.
echo Bundle Confirmation:
echo   When building without --bundles or --no-bundle flags, you will be prompted:
echo     [1] NSIS (.exe installer) [Default]
echo     [2] MSI / MSIX (.msi package)
echo     [3] Both (NSIS + MSI)
echo     [q] Cancel
echo.
echo Tauri Build Options (passed when building desktop app):
echo   --no-bundle           Skip installer packaging and produce only the release executable
echo   --debug               Build in debug mode instead of release
echo   --bundles [msi,nsis]  Specify installer bundle format (skips prompt)
echo   -v, --verbose         Enable verbose build output
echo.
echo Examples:
echo   build_app.bat
echo   build_app.bat all
echo   build_app.bat --bundles nsis
echo   build_app.bat --bundles msi
echo   build_app.bat --no-bundle
echo   build_app.bat web
echo.
exit /b 0

:audit_changelog
node scripts\changelog.mjs check
if !ERRORLEVEL! equ 0 exit /b 0
echo.
echo [ERROR] Changelog documentation is missing or out of date.
echo Review the files listed above and update CHANGELOG.json before continuing.
echo A checkpoint records file contents; it does not verify the accuracy of your notes.
if defined CI (
    echo [ERROR] Refresh the documentation checkpoint locally before running a CI build.
    exit /b 1
)
set "CHANGELOG_CONFIRM=N"
set /p "CHANGELOG_CONFIRM=Have you reviewed and updated the notes? Refresh checkpoint? [y/N]: "
if /i not "!CHANGELOG_CONFIRM!"=="y" (
    echo [INFO] Checkpoint unchanged. Update the notes, then rerun the build.
    exit /b 1
)
node scripts\changelog.mjs checkpoint
if !ERRORLEVEL! neq 0 (
    echo [ERROR] Checkpoint failed. Document every listed file before retrying.
    exit /b 1
)
node scripts\changelog.mjs check
if !ERRORLEVEL! neq 0 (
    echo [ERROR] Changelog audit still fails. The build will not continue.
    exit /b 1
)
echo [INFO] Documentation checkpoint refreshed and verified.
exit /b 0

:error_exit
echo.
echo [ERROR] Build process encountered an error.
if not defined CI if not defined NO_PAUSE pause
exit /b 1

:success_exit
if not defined CI if not defined NO_PAUSE pause
exit /b 0
