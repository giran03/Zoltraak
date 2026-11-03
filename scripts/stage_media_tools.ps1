param(
    [switch]$ValidateOnly,
    [string]$TargetDirectory = (Join-Path $PSScriptRoot '..\app\src-tauri\resources\bin'),
    [string]$ArchivePath = (Join-Path $env:TEMP 'zoltraak-ffmpeg-8.1-full-shared.zip')
)
$ErrorActionPreference = 'Stop'
$lockPath = Join-Path $PSScriptRoot 'media_tools.lock.json'
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
[void][Reflection.Assembly]::LoadWithPartialName('System.IO.Compression.FileSystem')
$extension = Join-Path $PSScriptRoot '..\app\src-tauri\resources\zoltraak-signed.xpi'
if (!(Test-Path -LiteralPath $extension -PathType Leaf)) { throw 'The bundled signed Firefox extension is missing.' }
$xpi = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($extension))
try {
    foreach ($name in @('manifest.json', 'META-INF/mozilla.rsa', 'META-INF/mozilla.sf')) {
        if (!$xpi.GetEntry($name)) { throw "Signed Firefox resource is incomplete: $name" }
    }
} finally { $xpi.Dispose() }
function Get-Sha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '').ToLower() }
    finally { $stream.Dispose(); $algorithm.Dispose() }
}
function Test-Payload([string]$Directory) {
    foreach ($entry in $lock.files) {
        $path = Join-Path $Directory $entry.name
        if (!(Test-Path -LiteralPath $path -PathType Leaf)) { return $false }
        if ((Get-Sha256 $path) -ne $entry.sha256) { return $false }
    }
    return $true
}
# The engine's own list of every encoder, filter, muxer and demuxer it uses.
# Engine tests keep it in step with the code, so a smaller FFmpeg build that
# passes this check runs every recipe.
$capabilities = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\engine\core\media_capabilities.json') -Raw | ConvertFrom-Json
function Assert-Listed([string]$Listing, [string]$Kind, [string[]]$Names) {
    foreach ($name in $Names) {
        # Lines look like " V....D libx264  ..." or " E  mp4  ..."; demuxer names
        # may be a comma list such as "mov,mp4,m4a".
        $pattern = '(?m)^\s*[A-Z.]{1,6}\s+(\S+,)?' + [regex]::Escape($name) + '(,\S+)?\s'
        if ($Listing -notmatch $pattern) { throw "Required media $Kind missing: $name" }
    }
}
function Assert-Capabilities([string]$Directory) {
    $ffmpeg = Join-Path $Directory 'ffmpeg.exe'
    $ffprobe = Join-Path $Directory 'ffprobe.exe'
    $expected = 'version ' + [regex]::Escape($lock.version) + '\b'
    $version = & $ffmpeg -version | Out-String
    if ($LASTEXITCODE -ne 0 -or $version -notmatch ('ffmpeg ' + $expected)) { throw 'Pinned FFmpeg could not run.' }
    $probeVersion = & $ffprobe -version | Out-String
    if ($LASTEXITCODE -ne 0 -or $probeVersion -notmatch ('ffprobe ' + $expected)) { throw 'Pinned FFprobe could not run.' }
    foreach ($kind in @('encoders', 'filters', 'muxers', 'demuxers')) {
        $listing = & $ffmpeg -hide_banner "-$kind" | Out-String
        if ($LASTEXITCODE -ne 0) { throw "Could not enumerate media $kind." }
        Assert-Listed $listing $kind.TrimEnd('s') $capabilities.$kind
    }
}
if (!(Test-Payload $TargetDirectory)) {
    if ($ValidateOnly) { throw 'Bundled media tools are incomplete or differ from the pinned build. Run scripts/stage_media_tools.ps1.' }
    if (!(Test-Path -LiteralPath $ArchivePath)) {
        Write-Host '[INFO] Downloading pinned FFmpeg full shared build...'
        Invoke-WebRequest -Uri $lock.url -OutFile $ArchivePath -UseBasicParsing
    }
    if ((Get-Sha256 $ArchivePath) -ne $lock.archiveSha256) { throw 'FFmpeg archive checksum mismatch. No resources were replaced.' }
    $temporaryRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
    $staging = Join-Path $temporaryRoot ('zoltraak-media-' + [guid]::NewGuid().ToString('N'))
    try {
        [IO.Compression.ZipFile]::ExtractToDirectory($ArchivePath, $staging)
        $package = Join-Path $staging $lock.directory
        $payload = Join-Path $staging 'payload'
        New-Item -ItemType Directory -Path $payload | Out-Null
        foreach ($entry in $lock.files) {
            $source = Join-Path $package $entry.name
            if (!(Test-Path -LiteralPath $source)) { $source = Join-Path (Join-Path $package 'bin') $entry.name }
            Copy-Item -LiteralPath $source -Destination (Join-Path $payload $entry.name)
        }
        if (!(Test-Payload $payload)) { throw 'Extracted media payload failed verification.' }
        Assert-Capabilities $payload
        New-Item -ItemType Directory -Force -Path $TargetDirectory | Out-Null
        foreach ($entry in $lock.files) { Copy-Item -LiteralPath (Join-Path $payload $entry.name) -Destination (Join-Path $TargetDirectory $entry.name) -Force }
    } finally {
        $resolved = [IO.Path]::GetFullPath($staging)
        if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Refusing to clean a staging directory outside TEMP.' }
        if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
    }
}
Assert-Capabilities $TargetDirectory
if (!$ValidateOnly) {
    $manifest = Join-Path $TargetDirectory 'media_tools.lock.json'
    if (!(Test-Path -LiteralPath $manifest) -or (Get-Sha256 $manifest) -ne (Get-Sha256 $lockPath)) {
        Copy-Item -LiteralPath $lockPath -Destination $manifest -Force
    }
}
$bytes = ($lock.files | ForEach-Object { (Get-Item -LiteralPath (Join-Path $TargetDirectory $_.name)).Length } | Measure-Object -Sum).Sum
Write-Host ('[SUCCESS] Both media tools, shared libraries and notices verified: {0:N2} MiB.' -f ($bytes / 1MB))
