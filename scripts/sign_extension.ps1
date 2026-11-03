<#
.SYNOPSIS
    Zoltraak Extension Signer & Multi-Target Deployer
.DESCRIPTION
    Signs the configured WebExtension manifest version via Mozilla AMO (unlisted),
    and synchronizes the signed XPI artifact across all Tauri resource locations and
    active browser policies (Zen Browser / Firefox).
#>

[CmdletBinding()]
param(
    [switch]$NoBump,
    [string]$ApiKey = $env:WEB_EXT_API_KEY,
    [string]$ApiSecret = $env:WEB_EXT_API_SECRET
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($ApiKey) -or [string]::IsNullOrWhiteSpace($ApiSecret)) { throw "Set WEB_EXT_API_KEY and WEB_EXT_API_SECRET before signing." }

# Resolve repo root
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$ExtDir = Join-Path $RepoRoot "extension"
$ManifestPath = Join-Path $ExtDir "manifest.json"

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "   Zoltraak Extension Signer & Bundler" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "Repository root: $RepoRoot"

if (-not (Test-Path $ManifestPath)) {
    throw "manifest.json not found at $ManifestPath"
}

# Clean up any stale upload UUID from previous aborted runs
$AmoUuidFile = Join-Path $ExtDir ".amo-upload-uuid"
if (Test-Path $AmoUuidFile) {
    Remove-Item $AmoUuidFile -Force -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# [1/4] Version Management
# ---------------------------------------------------------------------------
Write-Host "`n[1/4] Reading configured extension version..." -ForegroundColor Yellow

$ManifestContent = Get-Content -Path $ManifestPath -Raw
$NewVersion = ($ManifestContent | ConvertFrom-Json).version
if ($NewVersion -notmatch '^(0|[1-9]\d{0,4})(\.(0|[1-9]\d{0,4})){0,3}$') {
    throw 'Invalid extension version. Run scripts\bat\update_extension_version.bat first.'
}
$VersionParts = @($NewVersion.Split('.') | ForEach-Object { [int]$_ })
if (@($VersionParts | Where-Object { $_ -gt 65535 }).Count -gt 0 -or ($VersionParts | Measure-Object -Sum).Sum -eq 0) {
    throw 'Invalid extension version. Numbers must be 0-65535 and not all zero.'
}
# NoBump remains accepted for compatibility; signing never changes the version.
Write-Host "  [*] Using configured manifest version: $NewVersion" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# [2/4] Signing Extension via Mozilla AMO
# ---------------------------------------------------------------------------
Write-Host "`n[2/4] Signing extension via web-ext (AMO unlisted channel)..." -ForegroundColor Yellow
Write-Host "  Version to sign: $NewVersion"
Write-Host "  (Mozilla AMO automated signing may take 1-3 minutes; please wait...)" -ForegroundColor DarkGray

function Invoke-AmoSign {
    param(
        [string]$ApiKey,
        [string]$ApiSecret
    )
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $env:WEB_EXT_API_KEY = $ApiKey
        $env:WEB_EXT_API_SECRET = $ApiSecret
        $rawOutput = & npx.cmd web-ext sign `
            --channel=unlisted `
            --timeout=300000 `
            --artifacts-dir="web-ext-artifacts" 2>&1 | ForEach-Object { "$_" }
        
        $exitCode = $LASTEXITCODE
        return @{
            ExitCode = $exitCode
            Output   = $rawOutput
        }
    }
    finally {
        $ErrorActionPreference = $prevEAP
    }
}

Push-Location $ExtDir
try {
    $res = Invoke-AmoSign -ApiKey $ApiKey -ApiSecret $ApiSecret
    $SignExitCode = $res.ExitCode
    $SignOutput = $res.Output
    $SignOutput | ForEach-Object { Write-Host "  $_" }

    # Version changes belong to the version manager, including AMO conflicts.
    if ($SignExitCode -ne 0 -and ($SignOutput -match "already exists" -or $SignOutput -match "Conflict" -or $SignOutput -match "already been submitted")) {
        throw "Version $NewVersion already exists on AMO. Run scripts\bat\update_extension_version.bat to choose a higher version, then sign again."
    }

    if ($SignExitCode -ne 0) {
        throw "web-ext sign failed with exit code $SignExitCode"
    }
}
finally {
    Pop-Location
}

# ---------------------------------------------------------------------------
# [3/4] Verify Artifact and Deploy to Tauri Resources
# ---------------------------------------------------------------------------
Write-Host "`n[3/4] Locating signed XPI and deploying to resource targets..." -ForegroundColor Yellow

$ArtifactsDir = Join-Path $ExtDir "web-ext-artifacts"
$TargetArtifact = Get-ChildItem -Path $ArtifactsDir -Filter "*-$NewVersion.xpi" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $TargetArtifact) {
    throw "Could not find signed XPI artifact for version $NewVersion in $ArtifactsDir"
}

Write-Host "  [+] Verified signed artifact: $($TargetArtifact.Name) ($($TargetArtifact.Length) bytes)" -ForegroundColor Green

# Define all target locations that Tauri and browser policy look at
$DeploymentTargets = @(
    (Join-Path $RepoRoot "app\src-tauri\resources\zoltraak-signed.xpi"),
    (Join-Path $RepoRoot "app\src-tauri\target\debug\resources\zoltraak-signed.xpi"),
    (Join-Path $RepoRoot "app\src-tauri\target\release\resources\zoltraak-signed.xpi"),
    (Join-Path ([System.Environment]::GetFolderPath("ApplicationData")) "dev.zoltraak.app\extension\zoltraak-signed.xpi")
)

foreach ($Target in $DeploymentTargets) {
    $TargetDir = Split-Path $Target -Parent
    $ParentOfTargetDir = Split-Path $TargetDir -Parent
    
    # If the parent directory exists (e.g. app\src-tauri or target\debug or AppData), deploy there
    if (Test-Path $ParentOfTargetDir) {
        if (-not (Test-Path $TargetDir)) {
            New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null
        }
        Copy-Item -Path $TargetArtifact.FullName -Destination $Target -Force
        Write-Host "  [+] Deployed: $Target" -ForegroundColor Green
    }
}

# ---------------------------------------------------------------------------
# [4/4] Policy Check & Active Browser Guidance
# ---------------------------------------------------------------------------
Write-Host "`n[4/4] Checking browser enterprise policies..." -ForegroundColor Yellow

$KnownPolicyPaths = @(
    "G:\ZenBrowser\distribution\policies.json",
    "$env:LOCALAPPDATA\Programs\zen\distribution\policies.json",
    "$env:LOCALAPPDATA\Zen\distribution\policies.json",
    "C:\Program Files\Zen Browser\distribution\policies.json",
    "C:\Program Files\Zen\distribution\policies.json",
    "C:\Program Files\Mozilla Firefox\distribution\policies.json"
)

$PolicyUpdated = $false
foreach ($PolicyPath in $KnownPolicyPaths) {
    if (Test-Path $PolicyPath) {
        Write-Host "  [*] Found browser policy at: $PolicyPath" -ForegroundColor Cyan
        try {
            $PolicyJson = Get-Content $PolicyPath -Raw | ConvertFrom-Json
            $CurrentUrl = $PolicyJson.policies.ExtensionSettings.'zoltraak@zoltraak.app'.install_url
            if (-not $CurrentUrl) {
                $CurrentUrl = $PolicyJson.policies.ExtensionSettings.'opendm@opendm.app'.install_url
            }
            if ($CurrentUrl) {
                Write-Host "      install_url: $CurrentUrl" -ForegroundColor DarkGray
            }
        } catch {
            # Ignore parse issues if file is custom
        }
        $PolicyUpdated = $true
    }
}

# Check if Zen / Firefox processes are running
$RunningBrowsers = Get-Process -Name "zen*", "firefox*" -ErrorAction SilentlyContinue
if ($RunningBrowsers) {
    $BrowserNames = ($RunningBrowsers | Select-Object -ExpandProperty ProcessName -Unique) -join ", "
    Write-Host "`n[!] Active browser detected ($BrowserNames)." -ForegroundColor Magenta
    Write-Host "    To apply the updated extension (v$NewVersion):" -ForegroundColor Magenta
    Write-Host "    -> Restart the browser (Zen / Firefox auto-installs the new XPI on launch)," -ForegroundColor Magenta
    Write-Host "    -> OR navigate to about:addons and click 'Check for Updates'." -ForegroundColor Magenta
} else {
    Write-Host "`n[+] Browser is not currently running. It will load v$NewVersion automatically on next launch." -ForegroundColor Green
}

Write-Host "`n====================================================" -ForegroundColor Cyan
Write-Host "   Extension Signed & Deployed Successfully (v$NewVersion)!" -ForegroundColor Green
Write-Host "====================================================" -ForegroundColor Cyan
