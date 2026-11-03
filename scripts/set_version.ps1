# Zoltraak Version Manager
# Usage: powershell -File scripts\set_version.ps1 0.1.1

param (
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$NewVersion,
    [string]$ProjectRoot
)

$ErrorActionPreference = "Stop"
if (-not $ProjectRoot) { $ProjectRoot = Split-Path -Parent $PSScriptRoot }

# Basic SemVer validation (e.g. 0.1.1 or 1.0.0-rc1)
if ($NewVersion -notmatch '^\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$') {
    Write-Error "Invalid version format '$NewVersion'. Expected SemVer format (e.g. 0.1.1, 1.0.0)."
    exit 1
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$PreviousVersion = (Get-Content -LiteralPath (Join-Path $ProjectRoot 'app/package.json') -Raw | ConvertFrom-Json).version
$NotesChoice = '--keep'
$Changelog = Get-Content -LiteralPath (Join-Path $ProjectRoot 'CHANGELOG.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$PreviousNotes = $Changelog.releases.PSObject.Properties[$PreviousVersion].Value
if ($NewVersion -ne $PreviousVersion -and $PreviousNotes.status -eq 'development' -and $PreviousNotes.changes.Count -gt 0) {
    Write-Host "Detected $($PreviousNotes.changes.Count) logged development changes on v$PreviousVersion." -ForegroundColor Yellow
    while ($true) {
        Write-Host "Transfer these changes to v${NewVersion}? (y/n, q to cancel): " -NoNewline
        $answer = [Console]::ReadLine()
        if ($null -eq $answer -or $answer.Trim().ToLower() -in @('q','quit','cancel')) {
            Write-Host 'Version update cancelled. No files changed.'
            return
        }
        $answer = $answer.Trim().ToLower()
        if ($answer -in @('y','yes')) { $NotesChoice = '--transfer'; break }
        if ($answer -in @('n','no')) { break }
        Write-Host 'Enter y or n, or q to cancel.'
    }
}

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "  Updating Zoltraak Version to v$NewVersion" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan

# 1. Update app/package.json
$pkgPath = Join-Path $ProjectRoot "app\package.json"
if (Test-Path $pkgPath) {
    $content = Get-Content $pkgPath -Raw
    $content = $content -replace '("version":\s*")[^"]+(")', "`${1}$NewVersion`${2}"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($pkgPath, $content, $utf8NoBom)
    Write-Host "[OK] app\package.json -> $NewVersion" -ForegroundColor Green
}

# 2. Update app/src-tauri/tauri.conf.json
$tauriConfPath = Join-Path $ProjectRoot "app\src-tauri\tauri.conf.json"
if (Test-Path $tauriConfPath) {
    $content = Get-Content $tauriConfPath -Raw
    $content = $content -replace '("version":\s*")[^"]+(")', "`${1}$NewVersion`${2}"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($tauriConfPath, $content, $utf8NoBom)
    Write-Host "[OK] app\src-tauri\tauri.conf.json -> $NewVersion" -ForegroundColor Green
}

# 3. Update app/src-tauri/Cargo.toml
$tauriCargoPath = Join-Path $ProjectRoot "app\src-tauri\Cargo.toml"
if (Test-Path $tauriCargoPath) {
    $content = Get-Content $tauriCargoPath -Raw
    $content = $content -replace '(?m)^(version\s*=\s*")[^"]+(")', "`${1}$NewVersion`${2}"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($tauriCargoPath, $content, $utf8NoBom)
    Write-Host "[OK] app\src-tauri\Cargo.toml -> $NewVersion" -ForegroundColor Green
}

# 4. Update engine/Cargo.toml
$engineCargoPath = Join-Path $ProjectRoot "engine\Cargo.toml"
if (Test-Path $engineCargoPath) {
    $content = Get-Content $engineCargoPath -Raw
    $content = $content -replace '(?m)^(version\s*=\s*")[^"]+(")', "`${1}$NewVersion`${2}"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($engineCargoPath, $content, $utf8NoBom)
    Write-Host "[OK] engine\Cargo.toml -> $NewVersion" -ForegroundColor Green
}

Write-Host ""
& node (Join-Path $ScriptDir 'changelog.mjs') start $NewVersion $PreviousVersion $NotesChoice --root $ProjectRoot
if ($LASTEXITCODE -ne 0) { throw 'Could not start the development changelog entry.' }
Write-Host "All project files updated to v$NewVersion successfully!" -ForegroundColor Green
Write-Host "Document this version in CHANGELOG.json, then run node scripts/changelog.mjs checkpoint." -ForegroundColor Cyan
Write-Host "Next step: run 'scripts\bat\build_app.bat' to build and sign the v$NewVersion release." -ForegroundColor Cyan
Write-Host ""
