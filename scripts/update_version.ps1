# Zoltraak Version Manager
# Interactive version update script for Zoltraak App

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$pkgPath = Join-Path $ProjectRoot "app\package.json"

if (-not (Test-Path $pkgPath)) {
    Write-Host "[ERROR] Cannot find app\package.json at: $pkgPath" -ForegroundColor Red
    exit 1
}

# 1. Read current version
try {
    $pkgJson = Get-Content -Path $pkgPath -Raw | ConvertFrom-Json
    $CurrentVersion = $pkgJson.version
} catch {
    Write-Host "[ERROR] Failed to read version from app\package.json: $_" -ForegroundColor Red
    exit 1
}

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "  Zoltraak App Version Manager" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Current version: v$CurrentVersion" -ForegroundColor Yellow
Write-Host ""

# 2. Ask user (y/n) to change current version
$confirm = ""
while ($true) {
    Write-Host "Do you want to change the current version? (y/n): " -NoNewline -ForegroundColor White
    $confirm = [Console]::ReadLine()
    if ($null -eq $confirm) {
        Write-Host ""
        Write-Host "No input received. Exiting." -ForegroundColor Yellow
        exit 0
    }
    $confirm = $confirm.Trim().ToLower()

    if ($confirm -eq "n" -or $confirm -eq "no") {
        Write-Host ""
        Write-Host "Version update cancelled. No changes were made." -ForegroundColor Yellow
        exit 0
    }

    if ($confirm -eq "y" -or $confirm -eq "yes") {
        break
    }

    Write-Host "[!] Invalid input. Please enter 'y' for yes or 'n' for no.`n" -ForegroundColor Red
}

# Comparison function: returns 'HIGHER', 'LOWER', 'EQUAL', or 'INVALID_FORMAT'
function Compare-SemVer([string]$CurrentVer, [string]$NewVer) {
    if ($NewVer -notmatch '^\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$') {
        return "INVALID_FORMAT"
    }

    $cCore = ($CurrentVer -split '-')[0].Split('.') | ForEach-Object { [int]$_ }
    $nCore = ($NewVer -split '-')[0].Split('.') | ForEach-Object { [int]$_ }

    for ($i = 0; $i -lt 3; $i++) {
        if ($nCore[$i] -gt $cCore[$i]) { return "HIGHER" }
        if ($nCore[$i] -lt $cCore[$i]) { return "LOWER" }
    }

    $cHasPre = $CurrentVer -match '-'
    $nHasPre = $NewVer -match '-'

    if (-not $cHasPre -and $nHasPre) { return "LOWER" }
    if ($cHasPre -and -not $nHasPre) { return "HIGHER" }
    if ($cHasPre -and $nHasPre) {
        $cPre = ($CurrentVer -split '-', 2)[1]
        $nPre = ($NewVer -split '-', 2)[1]
        $cmp = [string]::Compare($nPre, $cPre, [System.StringComparison]::OrdinalIgnoreCase)
        if ($cmp -gt 0) { return "HIGHER" }
        if ($cmp -lt 0) { return "LOWER" }
    }

    return "EQUAL"
}

# 3. Prompt for version number with rejection and re-asking
$NewVersion = ""
Write-Host ""
while ($true) {
    Write-Host "Enter new version number (e.g. 0.1.8) or 'q' to cancel: " -NoNewline -ForegroundColor White
    $inputVersion = [Console]::ReadLine()
    if ($null -eq $inputVersion) {
        Write-Host ""
        Write-Host "No input received. Exiting." -ForegroundColor Yellow
        exit 0
    }
    $inputVersion = $inputVersion.Trim()

    if ($inputVersion -eq "" ) {
        Write-Host "[ERROR] Version number cannot be empty.`n" -ForegroundColor Red
        continue
    }

    if ($inputVersion -eq "q" -or $inputVersion -eq "quit" -or $inputVersion -eq "cancel") {
        Write-Host ""
        Write-Host "Version update cancelled. No changes were made." -ForegroundColor Yellow
        exit 0
    }

    $cmp = Compare-SemVer -CurrentVer $CurrentVersion -NewVer $inputVersion

    if ($cmp -eq "INVALID_FORMAT") {
        Write-Host "[ERROR] Invalid version format '$inputVersion'." -ForegroundColor Red
        Write-Host "        Expected Semantic Version format (e.g. 0.1.8, 1.0.0, 0.2.0-rc1).`n" -ForegroundColor Red
        continue
    }

    if ($cmp -eq "LOWER") {
        Write-Host "[ERROR] Version '$inputVersion' is lower than current version '$CurrentVersion'." -ForegroundColor Red
        Write-Host "        Rejected. Please enter a higher version number.`n" -ForegroundColor Yellow
        continue
    }

    if ($cmp -eq "EQUAL") {
        Write-Host "[ERROR] Version '$inputVersion' is already the current version ($CurrentVersion)." -ForegroundColor Red
        Write-Host "        Rejected. Please enter a higher version number.`n" -ForegroundColor Yellow
        continue
    }

    if ($cmp -eq "HIGHER") {
        $NewVersion = $inputVersion
        break
    }
}

# 4. Proceed to update project files via set_version.ps1
Write-Host ""
$setVersionScript = Join-Path $ScriptDir "set_version.ps1"
if (-not (Test-Path $setVersionScript)) {
    Write-Host "[ERROR] Cannot find set_version.ps1 at: $setVersionScript" -ForegroundColor Red
    exit 1
}

& $setVersionScript $NewVersion
if ($LASTEXITCODE -and $LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] set_version.ps1 failed with exit code $LASTEXITCODE" -ForegroundColor Red
    exit $LASTEXITCODE
}
