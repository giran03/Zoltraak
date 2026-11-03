<#
.SYNOPSIS
    Package Zoltraak Extension for the Chrome Web Store
.DESCRIPTION
    Creates a production-ready, store-compliant ZIP package for Chrome Web Store upload.
    - Strips Firefox-specific properties (browser_specific_settings, webRequestBlocking)
    - Bundles required extension icons (16, 48, 128)
    - Preserves deterministic Chromium developer key
    - Produces zoltraak-chrome-extension.zip ready for Developer Dashboard upload
#>

[CmdletBinding()]
param(
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$ExtDir = Join-Path $RepoRoot "extension"

if (-not $OutputDir) {
    $OutputDir = $ExtDir
}
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "   Zoltraak Chrome Web Store Packager" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan

# Prepare temp build directory
$TempBuild = Join-Path ([System.IO.Path]::GetTempPath()) "zoltraak-chrome-build-$([System.Guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Path $TempBuild -Force | Out-Null

try {
    # 1. Copy base extension files
    $FilesToCopy = @(
        "background.js",
        "content.js",
        "video-identity.js",
        "pill-placement.js",
        "facebook-post.js",
        "media-probe.js",
        "generic-media.js",
        "api.js",
        "theme.js",
        "popup.html",
        "popup.js",
        "popup.css",
        "placement.html",
        "placement.css",
        "placement.js",
        "placement-controller.js",
        "pill-controller.js",
        "intercept.html",
        "intercept.js",
        "yt-inject.js"
    )

    foreach ($file in $FilesToCopy) {
        $sourcePath = Join-Path $ExtDir $file
        if (Test-Path $sourcePath) {
            Copy-Item -Path $sourcePath -Destination (Join-Path $TempBuild $file) -Force
        }
    }

    # Copy fonts
    $FontsSource = Join-Path $ExtDir "fonts"
    if (Test-Path $FontsSource) {
        Copy-Item -Path $FontsSource -Destination (Join-Path $TempBuild "fonts") -Recurse -Force
    }

    # Copy / generate icons
    $TempIcons = Join-Path $TempBuild "icons"
    New-Item -ItemType Directory -Path $TempIcons -Force | Out-Null

    # extension/icons holds the Zoltraak default icon at every size the manifest
    # declares, so Firefox and Chromium packages ship the same artwork.
    Copy-Item -Path (Join-Path $ExtDir "icons\*.png") -Destination $TempIcons -Force

    # 2. Prepare store-compliant manifest.json
    $ManifestJson = Get-Content (Join-Path $ExtDir "manifest.json") -Raw | ConvertFrom-Json

    # Remove Firefox-specific fields
    if ($ManifestJson.PSObject.Properties['browser_specific_settings']) {
        $ManifestJson.PSObject.Properties.Remove('browser_specific_settings')
    }

    # Remove webRequestBlocking (forbidden in Chrome MV3 Web Store)
    if ($ManifestJson.permissions) {
        $ManifestJson.permissions = @($ManifestJson.permissions | Where-Object { $_ -ne "webRequestBlocking" })
    }

    # Ensure 'key' is removed (Chrome Web Store strictly forbids 'key' in manifest.json for store uploads)
    if ($ManifestJson.PSObject.Properties['key']) {
        $ManifestJson.PSObject.Properties.Remove('key')
    }
    $ManifestJson | Add-Member -MemberType NoteProperty -Name "minimum_chrome_version" -Value "119" -Force

    # Ensure background uses service_worker for Chrome MV3
    $ManifestJson.background = [PSCustomObject]@{
        "service_worker" = "background.js"
        "type" = "module"
    }

    # Add icons to manifest
    $IconsObject = [PSCustomObject]@{
        "16" = "icons/16.png"
        "32" = "icons/32.png"
        "48" = "icons/48.png"
        "128" = "icons/128.png"
    }
    $ManifestJson | Add-Member -MemberType NoteProperty -Name "icons" -Value $IconsObject -Force
    if ($ManifestJson.action) {
        $ManifestJson.action | Add-Member -MemberType NoteProperty -Name "default_icon" -Value $IconsObject -Force
    }

    $ManifestOutPath = Join-Path $TempBuild "manifest.json"
    $ManifestText = $ManifestJson | ConvertTo-Json -Depth 10
    $Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($ManifestOutPath, $ManifestText, $Utf8NoBom)

    # 3. Create zip archive
    $ZipDestination = Join-Path $OutputDir "zoltraak-chrome-extension.zip"
    if (Test-Path $ZipDestination) {
        Remove-Item $ZipDestination -Force
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory($TempBuild, $ZipDestination)

    Write-Host "`n[+] Successfully created Chrome Web Store package:" -ForegroundColor Green
    Write-Host "    $ZipDestination" -ForegroundColor Cyan
    Write-Host "    Package size: $((Get-Item $ZipDestination).Length) bytes"
    Write-Host "    Version: $($ManifestJson.version)"
}
finally {
    if (Test-Path $TempBuild) {
        $CleanupRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        $CleanupTarget = (Resolve-Path -LiteralPath $TempBuild).Path
        if (-not $CleanupTarget.StartsWith($CleanupRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not (Split-Path -Leaf $CleanupTarget).StartsWith('zoltraak-chrome-build-')) {
            throw "Refusing to remove an unexpected packaging directory."
        }
        Remove-Item -LiteralPath $CleanupTarget -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "====================================================" -ForegroundColor Cyan
