# Generate / manage Tauri updater Minisign keys for Zoltraak
$ErrorActionPreference = "Stop"

$KeyDir = "$HOME\.tauri"
$KeyPath = "$KeyDir\zoltraak.key"
$PubPath = "$KeyDir\zoltraak.key.pub"

if (-not (Test-Path $KeyDir)) {
    New-Item -ItemType Directory -Path $KeyDir -Force | Out-Null
}

$ProjectRoot = (Resolve-Path "$PSScriptRoot\..").Path
$AppDir = Join-Path $ProjectRoot "app"

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "  Zoltraak - Updater Keypair Generator (Minisign)  " -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan
Write-Host ""

if ((Test-Path $KeyPath) -and (Test-Path $PubPath)) {
    Write-Host "[INFO] Existing updater keypair found at:" -ForegroundColor Green
    Write-Host "  Private: $KeyPath" -ForegroundColor Gray
    Write-Host "  Public:  $PubPath" -ForegroundColor Gray
    $pub = (Get-Content $PubPath -Raw).Trim()
    Write-Host ""
    Write-Host "Public Key (for tauri.conf.json):" -ForegroundColor Yellow
    Write-Host $pub -ForegroundColor White
    exit 0
}

if ([string]::IsNullOrEmpty($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD)) { Write-Error "Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD to the password for the new key."; exit 1 }
Write-Host "[INFO] Generating new keypair..." -ForegroundColor Cyan
Push-Location $AppDir
try {
    cmd.exe /c "npx tauri signer generate -w %USERPROFILE%\.tauri\zoltraak.key --ci -p %TAURI_SIGNING_PRIVATE_KEY_PASSWORD% -f"
    if (Test-Path $PubPath) {
        $pub = (Get-Content $PubPath -Raw).Trim()
        Write-Host ""
        Write-Host "[SUCCESS] Keypair generated successfully!" -ForegroundColor Green
        Write-Host "Public Key: $pub" -ForegroundColor White
    }
} finally {
    Pop-Location
}

exit 0
