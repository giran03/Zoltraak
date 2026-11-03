# Run with: powershell -NoProfile -File scripts/tests/extension_version.test.ps1
$ErrorActionPreference = 'Stop'
$Fixture = Join-Path ([IO.Path]::GetTempPath()) ('zoltraak-extension-version-' + [guid]::NewGuid())
$OriginalPath = $env:PATH
$OriginalMode = $env:EXTENSION_TEST_SIGN_MODE
$OriginalAppData = $env:EXTENSION_TEST_APPDATA
foreach ($Directory in @('scripts', 'extension', 'tools', 'app\src-tauri', 'test-appdata\dev.zoltraak.app')) {
    New-Item -ItemType Directory -Path (Join-Path $Fixture $Directory) -Force | Out-Null
}
Copy-Item -LiteralPath "$PSScriptRoot\..\update_extension_version.ps1" -Destination "$Fixture\scripts"
# Redirect the copied signer's installed-app resource destination into the fixture.
$Signer = [IO.File]::ReadAllText("$PSScriptRoot\..\sign_extension.ps1").Replace('[System.Environment]::GetFolderPath("ApplicationData")', '$env:EXTENSION_TEST_APPDATA')
[IO.File]::WriteAllText("$Fixture\scripts\sign_extension.ps1", $Signer)
[IO.File]::WriteAllText("$Fixture\extension\manifest.json", '{"manifest_version":3,"name":"fixture","version":"0.1.45"}')
[IO.File]::WriteAllText("$Fixture\tools\npx.cmd", "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0fake_sign.ps1`"`r`nexit /b %ERRORLEVEL%`r`n")
[IO.File]::WriteAllText("$Fixture\tools\fake_sign.ps1", @'
$Root = Split-Path $PSScriptRoot -Parent
Add-Content "$Root\sign-calls.txt" 'called'
if ($env:EXTENSION_TEST_SIGN_MODE -eq 'conflict') { Write-Output 'Version already exists'; exit 1 }
$Version = (Get-Content "$Root\extension\manifest.json" -Raw | ConvertFrom-Json).version
if ($env:EXTENSION_TEST_SIGN_MODE -eq 'mismatch') { $Version = '9.9.9' }
New-Item -ItemType Directory "$Root\extension\web-ext-artifacts" -Force | Out-Null
[IO.File]::WriteAllText("$Root\extension\web-ext-artifacts\fixture-$Version.xpi", 'mock signed artifact')
exit 0
'@)

function Run-Script([string]$Name, [string]$Arguments, [string]$InputText = '') {
    $Start = New-Object Diagnostics.ProcessStartInfo
    $Start.FileName = 'powershell.exe'
    $Start.Arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$Fixture\scripts\$Name`" $Arguments"
    $Start.UseShellExecute = $false
    $Start.CreateNoWindow = $true
    $Start.RedirectStandardInput = $true
    $Start.RedirectStandardOutput = $true
    $Start.RedirectStandardError = $true
    $Process = [Diagnostics.Process]::Start($Start)
    $Process.StandardInput.Write($InputText)
    $Process.StandardInput.Close()
    if (-not $Process.WaitForExit(30000)) { $Process.Kill(); throw 'Fixture script timed out.' }
    $Output = $Process.StandardOutput.ReadToEnd() + $Process.StandardError.ReadToEnd()
    $Code = $Process.ExitCode
    $Process.Dispose()
    return @{ Code = $Code; Output = $Output }
}
function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Manifest-Version { return (Get-Content "$Fixture\extension\manifest.json" -Raw | ConvertFrom-Json).version }

try {
    $env:PATH = "$Fixture\tools;$OriginalPath"
    $env:EXTENSION_TEST_APPDATA = "$Fixture\test-appdata"
    $Before = [IO.File]::ReadAllText("$Fixture\extension\manifest.json")
    $Result = Run-Script 'update_extension_version.ps1' '' "n`n"
    Assert ($Result.Code -eq 0 -and [IO.File]::ReadAllText("$Fixture\extension\manifest.json") -eq $Before) 'Cancellation changed the manifest.'
    $Result = Run-Script 'update_extension_version.ps1' '' "y`n0.1.44`n0.1.46`n"
    Assert ($Result.Code -eq 0 -and (Manifest-Version) -eq '0.1.46') 'Interactive retry/update failed.'
    foreach ($Invalid in @('0.1.46', '0.1.45', '0.01.47', '0.1.47-rc1', '0.1.65536', '0.0.0', '0.1.46.0')) {
        $Result = Run-Script 'update_extension_version.ps1' "-Version $Invalid"
        Assert ($Result.Code -ne 0 -and (Manifest-Version) -eq '0.1.46') "Accepted invalid/non-increasing version: $Invalid"
    }
    $Result = Run-Script 'update_extension_version.ps1' '-Version 0.1.46.1'
    Assert ($Result.Code -eq 0 -and (Manifest-Version) -eq '0.1.46.1') 'Four-part update failed.'
    $Result = Run-Script 'sign_extension.ps1' '-ApiKey test -ApiSecret test'
    Assert ($Result.Code -eq 0 -and (Manifest-Version) -eq '0.1.46.1') 'Default signing changed the configured version.'
    Assert (Test-Path "$Fixture\app\src-tauri\resources\zoltraak-signed.xpi") 'Exact-version artifact was not deployed.'
    $env:EXTENSION_TEST_SIGN_MODE = 'conflict'
    $Result = Run-Script 'sign_extension.ps1' '-NoBump -ApiKey test -ApiSecret test'
    Assert ($Result.Code -ne 0 -and (Manifest-Version) -eq '0.1.46.1') 'Signing conflict changed the configured version.'
    Assert ((Get-Content "$Fixture\sign-calls.txt").Count -eq 2) 'Signing conflict retried instead of requiring a version update.'
    $env:EXTENSION_TEST_SIGN_MODE = 'mismatch'
    $Result = Run-Script 'update_extension_version.ps1' '-Version 0.1.47'
    Assert ($Result.Code -eq 0) 'Fixture version update failed.'
    $Result = Run-Script 'sign_extension.ps1' '-ApiKey test -ApiSecret test'
    Assert ($Result.Code -ne 0 -and $Result.Output.Contains('Could not find signed XPI artifact for version 0.1.47')) 'Signer accepted a different-version artifact.'
    Write-Host 'Passed: cancellation, interactive retry, version validation, exact-version signing, conflict handling, and artifact matching.'
} finally {
    $env:PATH = $OriginalPath
    $env:EXTENSION_TEST_SIGN_MODE = $OriginalMode
    $env:EXTENSION_TEST_APPDATA = $OriginalAppData
    $ResolvedFixture = [IO.Path]::GetFullPath($Fixture)
    $TempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if (-not $ResolvedFixture.StartsWith($TempRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $ResolvedFixture -Leaf) -notlike 'zoltraak-extension-version-*') { throw 'Unsafe fixture cleanup path.' }
    Remove-Item -LiteralPath $ResolvedFixture -Recurse -Force
}
