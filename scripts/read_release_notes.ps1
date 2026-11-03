param([string]$ProjectRoot = (Split-Path -Parent $PSScriptRoot), [switch]$ValidateOnly)

$ErrorActionPreference = 'Stop'
$config = Get-Content -LiteralPath (Join-Path $ProjectRoot 'app/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$package = Get-Content -LiteralPath (Join-Path $ProjectRoot 'app/package.json') -Raw | ConvertFrom-Json
if ($config.version -ne $package.version) { throw 'App and Tauri versions differ. Run the app version script before building.' }
$source = Get-Content -LiteralPath (Join-Path $ProjectRoot 'CHANGELOG.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ($source.schemaVersion -ne 1) { throw 'Unsupported CHANGELOG.json schemaVersion.' }
$releaseProperty = $source.releases.PSObject.Properties[$config.version]
if (-not $releaseProperty) { throw "Add release notes for version $($config.version) to CHANGELOG.json before building." }
$release = $releaseProperty.Value
if ($release.summary -isnot [string] -or [string]::IsNullOrWhiteSpace($release.summary)) { throw 'Release summary must be a nonempty string.' }
if ($release.changes -isnot [array] -or $release.changes.Count -eq 0) { throw 'Release changes must be a nonempty array.' }
foreach ($change in $release.changes) {
    if ($change.brief -isnot [string] -or [string]::IsNullOrWhiteSpace($change.brief)) { throw 'Each change needs a nonempty brief string.' }
    if ($change.details -isnot [array] -or $change.details.Count -eq 0) { throw 'Each change needs a nonempty details array.' }
    foreach ($detail in $change.details) {
        if ($detail -isnot [string] -or [string]::IsNullOrWhiteSpace($detail)) { throw 'Each detailed note must be a nonempty string.' }
    }
}
if ($ValidateOnly) { Write-Host "Release notes validated for v$($config.version)."; return }
$brief = $release.summary + "`n`n" + (($release.changes | ForEach-Object { '- ' + $_.brief }) -join "`n")
$detailed = $release.summary + "`n`n" + (($release.changes | ForEach-Object {
    '### ' + $_.brief + "`n" + (($_.details | ForEach-Object { '- ' + $_ }) -join "`n")
}) -join "`n`n")
[pscustomobject]@{ version = $config.version; brief = $brief; detailed = $detailed; release = $release }
