# Zoltraak Updater Manifest Generator (latest.json)
# Scans the target release bundle, signs the installer if needed, and writes latest.json

param(
    [string]$ProjectRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$BundleDir
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$TauriConfPath = Join-Path $ProjectRoot "app\src-tauri\tauri.conf.json"
if (-not $BundleDir) { $BundleDir = Join-Path $ProjectRoot "app\src-tauri\target\release\bundle" }
$NsisDir = Join-Path $BundleDir "nsis"
$KeyPath = Join-Path $env:USERPROFILE ".tauri\zoltraak.key"
$Password = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD

if (-not (Test-Path $TauriConfPath)) {
    Write-Error "tauri.conf.json not found at: $TauriConfPath"
    exit 1
}

$tauriConfig = Get-Content $TauriConfPath -Raw | ConvertFrom-Json
$version = $tauriConfig.version
$productName = $tauriConfig.productName
$releaseNotes = & (Join-Path $ScriptDir 'read_release_notes.ps1') -ProjectRoot $ProjectRoot
if (-not $version) { $version = "0.1.0" }
if (-not $productName) { $productName = "Zoltraak" }

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "  Generating latest.json for $productName v$version" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan

if (-not (Test-Path $NsisDir)) {
    Write-Host "[INFO] NSIS bundle directory not found at: $NsisDir (skipping latest.json updater manifest for non-NSIS build)." -ForegroundColor Yellow
    exit 0
}

# Find the installer exe
$installer = Get-ChildItem -Path $NsisDir -Filter "*setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) {
    # Fallback to any exe in nsis dir
    $installer = Get-ChildItem -Path $NsisDir -Filter "*.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

if (-not $installer) {
    Write-Error "No installer .exe found in $NsisDir. Ensure tauri build completed successfully."
    exit 1
}

$installerName = $installer.Name
$sigFile = Join-Path $NsisDir "$installerName.sig"

Write-Host "[INFO] Installer: $installerName" -ForegroundColor Gray

# If .sig doesn't exist, sign it now
if (-not (Test-Path $sigFile)) {
    if (Test-Path $KeyPath) {
        if ([string]::IsNullOrEmpty($Password)) { Write-Error "Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD to sign the installer."; exit 1 }
        Write-Host "[INFO] Signing installer with $KeyPath..." -ForegroundColor Yellow
        Push-Location (Join-Path $ProjectRoot "app")
        try {
            npx tauri signer sign -f $KeyPath -p $Password (Join-Path $NsisDir $installerName)
        } finally {
            Pop-Location
        }
    } else {
        Write-Error "Signature file $sigFile not found and signing key $KeyPath not found."
        exit 1
    }
}

if (-not (Test-Path $sigFile)) {
    Write-Error "Signature file not found: $sigFile"
    exit 1
}

$signature = (Get-Content $sigFile -Raw).Trim()

# Derive asset hosting from the same endpoint used by installed apps.
$endpoint = @($tauriConfig.plugins.updater.endpoints)[0]
if ($endpoint -notmatch '^https://github\.com/([^/]+/[^/]+)/releases/latest/download/latest\.json$') {
    throw 'Configure a GitHub latest.json updater endpoint before generating release assets.'
}
$repo = $matches[1]

$downloadUrl = "https://github.com/$repo/releases/download/v$version/$([uri]::EscapeDataString($installerName))"
$pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

$latestObj = [ordered]@{
    version = $version
    notes = $releaseNotes.detailed
    changelog = [ordered]@{
        brief = $releaseNotes.brief
        detailed = $releaseNotes.detailed
        summary = $releaseNotes.release.summary
        changes = $releaseNotes.release.changes
        status = $releaseNotes.release.status
        previous_version = $releaseNotes.release.previousVersion
        updated_at = $releaseNotes.release.updatedAt
    }
    pub_date = $pubDate
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = $signature
            url = $downloadUrl
        }
    }
}

$latestJsonContent = $latestObj | ConvertTo-Json -Depth 10

# Write latest.json to bundle root and nsis folder (UTF-8 WITHOUT BOM)
$outPath1 = Join-Path $BundleDir "latest.json"
$outPath2 = Join-Path $NsisDir "latest.json"

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($outPath1, $latestJsonContent, $utf8NoBom)
[System.IO.File]::WriteAllText($outPath2, $latestJsonContent, $utf8NoBom)

Write-Host ""
Write-Host "SUCCESS! Generated latest.json:" -ForegroundColor Green
Write-Host "  -> $outPath1" -ForegroundColor Green
Write-Host ""
Write-Host "Release Asset Checklist for GitHub (tag: v$version):" -ForegroundColor Cyan
Write-Host "  1. $installerName"
Write-Host "  2. $installerName.sig"
Write-Host "  3. latest.json"
Write-Host ""
