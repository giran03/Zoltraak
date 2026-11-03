# Interactive extension version manager. Optional argument: -Version 0.1.50
[CmdletBinding()]
param([Parameter(Position = 0)][string]$Version)

$ErrorActionPreference = 'Stop'
$ManifestPath = Join-Path $PSScriptRoot '..\extension\manifest.json'
$ManifestContent = [IO.File]::ReadAllText($ManifestPath)
$CurrentVersion = ($ManifestContent | ConvertFrom-Json).version

function ConvertTo-ExtensionVersion([string]$Value) {
    if ($Value -notmatch '^(0|[1-9]\d{0,4})(\.(0|[1-9]\d{0,4})){0,3}$') {
        throw 'Use 1 to 4 dot-separated numbers, without leading zeros or prerelease suffixes (e.g. 0.1.50).'
    }
    $Parts = @($Value.Split('.') | ForEach-Object { [int]$_ })
    if (@($Parts | Where-Object { $_ -gt 65535 }).Count -gt 0 -or ($Parts | Measure-Object -Sum).Sum -eq 0) {
        throw 'Each number must be between 0 and 65535, and the version cannot be all zeros.'
    }
    while ($Parts.Count -lt 4) { $Parts += 0 }
    return [version]($Parts -join '.')
}

$Current = ConvertTo-ExtensionVersion $CurrentVersion
Write-Host '====================================================' -ForegroundColor Cyan
Write-Host '  Zoltraak Extension Version Manager' -ForegroundColor Cyan
Write-Host '====================================================' -ForegroundColor Cyan
Write-Host "Current version: v$CurrentVersion" -ForegroundColor Yellow

$Interactive = -not $PSBoundParameters.ContainsKey('Version')
if ($Interactive) {
    while ($true) {
        Write-Host 'Do you want to change the current version? (y/n): ' -NoNewline
        $Confirm = [Console]::ReadLine()
        if ($null -eq $Confirm -or $Confirm.Trim().ToLower() -in @('n', 'no')) {
            Write-Host 'Version update cancelled. No changes were made.'
            exit 0
        }
        if ($Confirm.Trim().ToLower() -in @('y', 'yes')) { break }
        Write-Host "[!] Please enter 'y' or 'n'." -ForegroundColor Yellow
    }
}

while ($true) {
    if ($Interactive) {
        Write-Host "Enter new version number (e.g. 0.1.50) or 'q' to cancel: " -NoNewline
        $Version = [Console]::ReadLine()
        if ($null -eq $Version -or $Version.Trim().ToLower() -in @('q', 'quit', 'cancel')) {
            Write-Host 'Version update cancelled. No changes were made.'
            exit 0
        }
    }
    try {
        $Version = $Version.Trim()
        $NewVersion = ConvertTo-ExtensionVersion $Version
        if ($NewVersion -le $Current) { throw "Version '$Version' must be higher than '$CurrentVersion'." }
        break
    } catch {
        if (-not $Interactive) { throw }
        Write-Host "[ERROR] $_" -ForegroundColor Red
    }
}

# Replace only the manifest version and preserve formatting and UTF-8 encoding.
$UpdatedContent = $ManifestContent -replace '("version"\s*:\s*")[^"]+(")', "`${1}$Version`${2}"
[IO.File]::WriteAllText($ManifestPath, $UpdatedContent, [Text.UTF8Encoding]::new($false))
Write-Host "[OK] extension\manifest.json -> $Version" -ForegroundColor Green
Write-Host 'Next step: run scripts\bat\sign_extension.bat to sign this exact version.' -ForegroundColor Cyan
