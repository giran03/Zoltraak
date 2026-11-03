$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('zoltraak-release-notes-' + [guid]::NewGuid())
function Assert($condition, $message) { if (-not $condition) { throw $message } }
try {
    New-Item -ItemType Directory -Path (Join-Path $fixture 'app/src-tauri'), (Join-Path $fixture 'bundle/nsis') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $fixture 'app/package.json') -Value '{"version":"0.2.5"}'
    Set-Content -LiteralPath (Join-Path $fixture 'app/src-tauri/tauri.conf.json') -Value '{"version":"0.2.5","productName":"Zoltraak","plugins":{"updater":{"endpoints":["https://github.com/giran03/Zoltraak/releases/latest/download/latest.json"]}}}'
    $realNotes = Get-Content -LiteralPath (Join-Path $project 'CHANGELOG.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $fixtureNotes = @{ schemaVersion = 1; releases = @{ '0.2.5' = $realNotes.releases.PSObject.Properties[$realNotes.development.version].Value } }
    $fixtureNotes | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $fixture 'CHANGELOG.json') -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $fixture 'bundle/nsis/Zoltraak_0.2.5_x64-setup.exe') -Value 'test installer only'
    Set-Content -LiteralPath (Join-Path $fixture 'bundle/nsis/Zoltraak_0.2.5_x64-setup.exe.sig') -Value 'fixture-signature'
    $notes = & (Join-Path $project 'scripts/read_release_notes.ps1') -ProjectRoot $fixture
    & (Join-Path $project 'scripts/generate_latest_json.ps1') -ProjectRoot $fixture -BundleDir (Join-Path $fixture 'bundle')
    $manifestPath = Join-Path $fixture 'bundle/latest.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert ($manifest.version -eq '0.2.5') 'Manifest version differs.'
    Assert ($manifest.notes -eq $notes.detailed) 'Updater notes must include all detailed notes.'
    Assert ($manifest.changelog.brief -eq $notes.brief) 'Brief notes differ.'
    Assert ($manifest.changelog.detailed -eq $notes.detailed) 'Detailed notes differ.'
    Assert ($manifest.changelog.changes.Count -eq $notes.release.changes.Count) 'Structured changes missing.'
    Assert ($manifest.platforms.'windows-x86_64'.signature -eq 'fixture-signature') 'Signature changed.'
    Assert ($manifest.platforms.'windows-x86_64'.url -match '/v0.2.5/Zoltraak_0.2.5_x64-setup.exe$') 'Installer URL changed.'
    Assert ($manifest.platforms.'windows-x86_64'.url -eq 'https://github.com/giran03/Zoltraak/releases/download/v0.2.5/Zoltraak_0.2.5_x64-setup.exe') 'Installer must use the configured standalone repository.'
    Assert ((Get-Content -LiteralPath $manifestPath -Raw) -eq (Get-Content -LiteralPath (Join-Path $fixture 'bundle/nsis/latest.json') -Raw)) 'Manifest copies differ.'
    $bytes = [IO.File]::ReadAllBytes($manifestPath)
    Assert (-not ($bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191)) 'Manifest has UTF8 BOM.'
    Set-Content -LiteralPath (Join-Path $fixture 'app/src-tauri/tauri.conf.json') -Value '{"version":"0.2.5","productName":"Zoltraak","plugins":{"updater":{"endpoints":["https://github.com/example/another-app/releases/latest/download/latest.json"]}}}'
    & (Join-Path $project 'scripts/generate_latest_json.ps1') -ProjectRoot $fixture -BundleDir (Join-Path $fixture 'bundle')
    $alternate = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert ($alternate.platforms.'windows-x86_64'.url -like 'https://github.com/example/another-app/releases/*') 'Asset hosting must follow the endpoint instead of a hardcoded repository.'
    Set-Content -LiteralPath (Join-Path $fixture 'app/src-tauri/tauri.conf.json') -Value '{"version":"0.2.5","productName":"Zoltraak"}'
    $rejected = $false
    try { & (Join-Path $project 'scripts/generate_latest_json.ps1') -ProjectRoot $fixture -BundleDir (Join-Path $fixture 'bundle') | Out-Null } catch { $rejected = $true }
    Assert $rejected 'Missing updater endpoint must fail instead of silently selecting a repository.'
    Set-Content -LiteralPath (Join-Path $fixture 'app/package.json') -Value '{"version":"0.2.6"}'
    $rejected = $false
    try { & (Join-Path $project 'scripts/read_release_notes.ps1') -ProjectRoot $fixture | Out-Null } catch { $rejected = $true }
    Assert $rejected 'Version mismatch must fail.'
    Set-Content -LiteralPath (Join-Path $fixture 'app/src-tauri/tauri.conf.json') -Value '{"version":"0.2.6"}'
    $rejected = $false
    try { & (Join-Path $project 'scripts/read_release_notes.ps1') -ProjectRoot $fixture | Out-Null } catch { $rejected = $true }
    Assert $rejected 'Missing release must fail.'
    Set-Content -LiteralPath (Join-Path $fixture 'CHANGELOG.json') -Value '{"schemaVersion":1,"releases":{"0.2.6":{"summary":"Test","changes":[{"brief":"Broken","details":[]}]}}}'
    $rejected = $false
    try { & (Join-Path $project 'scripts/read_release_notes.ps1') -ProjectRoot $fixture | Out-Null } catch { $rejected = $true }
    Assert $rejected 'Empty detailed notes must fail.'
    Write-Host 'Passed: manifest parity, signature/URL preservation, UTF8, and missing/invalid release validation.'
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path $resolved -Leaf).StartsWith('zoltraak-release-notes-')) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
