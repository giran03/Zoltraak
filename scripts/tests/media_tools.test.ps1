# Run with powershell -NoProfile -File scripts/tests/media_tools.test.ps1
param([string]$BinDirectory = (Join-Path $PSScriptRoot '..\..\app\src-tauri\resources\bin'))
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$stage = Join-Path $root 'scripts\stage_media_tools.ps1'
$bin = [IO.Path]::GetFullPath($BinDirectory)
$ffmpeg = Join-Path $bin 'ffmpeg.exe'
$ffprobe = Join-Path $bin 'ffprobe.exe'
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
$fixture = Join-Path $tempRoot ('zoltraak-media-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
function Run-Media([string[]]$Arguments) {
    $log = Join-Path $fixture 'ffmpeg-error.txt'
    & $ffmpeg -hide_banner -loglevel error -y @Arguments 2> $log
    if ($LASTEXITCODE -ne 0) { throw "Media command failed: $(Get-Content -LiteralPath $log -Raw)" }
}
function Assert-Media([string]$File, [string]$Codec = '') {
    $json = & $ffprobe -v quiet -show_streams -show_format -of json $File | Out-String | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or !$json.streams.Count -or [double]$json.format.duration -le 0) { throw "Invalid media output: $File" }
    if ($Codec -and !($json.streams | Where-Object codec_name -EQ $Codec)) { throw "Expected codec $Codec in $File" }
    return $json
}
try {
    & powershell.exe -NoProfile -File $stage -ValidateOnly -TargetDirectory $bin
    if ($LASTEXITCODE -ne 0) { throw 'Complete payload verification failed.' }
    $missing = Join-Path $fixture 'missing'
    New-Item -ItemType Directory -Path $missing | Out-Null
    Set-Content -LiteralPath (Join-Path $missing 'ffmpeg.exe') -Value 'incomplete'
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -File $stage -ValidateOnly -TargetDirectory $missing *> (Join-Path $fixture 'expected-failure.txt')
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -eq 0) { throw 'Incomplete media payload was accepted.' }
    $badArchive = Join-Path $fixture 'corrupt.zip'
    Set-Content -LiteralPath $badArchive -Value 'invalid archive'
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -File $stage -TargetDirectory $missing -ArchivePath $badArchive *> (Join-Path $fixture 'expected-failure.txt')
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -eq 0) { throw 'Unverified archive was accepted.' }
    if ((Get-Content -LiteralPath (Join-Path $missing 'ffmpeg.exe') -Raw).Trim() -ne 'incomplete') { throw 'Failed staging changed the existing payload.' }

    $source = Join-Path $fixture 'source.mp4'
    Run-Media @('-f','lavfi','-i','testsrc2=size=64x48:rate=10','-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-t','0.3','-c:v','libx264','-pix_fmt','yuv420p','-c:a','aac',$source)
    $null = Assert-Media $source 'h264'
    foreach ($target in @(@('mp3','libmp3lame','mp3'), @('m4a','aac','aac'), @('opus','libopus','opus'), @('ogg','libvorbis','vorbis'), @('flac','flac','flac'), @('wav','pcm_s16le','pcm_s16le'), @('alac.m4a','alac','alac'))) {
        $out = Join-Path $fixture ('audio.' + $target[0])
        Run-Media @('-i',$source,'-vn','-c:a',$target[1],'-metadata','title=Zoltraak smoke','-metadata','artist=Test artist',$out)
        $null = Assert-Media $out $target[2]
    }
    $cover = Join-Path $fixture 'cover.jpg'
    Run-Media @('-i',$source,'-frames:v','1','-vf','crop=min(iw\,ih):min(iw\,ih),scale=48:48,setsar=1','-c:v','mjpeg',$cover)
    $tagged = Join-Path $fixture 'tagged.mp3'
    Run-Media @('-i',$source,'-i',$cover,'-map','0:a:0','-map','1:v:0','-c:a','libmp3lame','-c:v','mjpeg','-disposition:v','attached_pic','-metadata','title=Zoltraak smoke',$tagged)
    $tags = Assert-Media $tagged 'mp3'
    if ($tags.format.tags.title -ne 'Zoltraak smoke' -or !($tags.streams | Where-Object { $_.disposition.attached_pic -eq 1 })) { throw 'Music tags/artwork were lost.' }
    foreach ($extension in @('mkv','mov')) {
        $out = Join-Path $fixture ('copy.' + $extension)
        Run-Media @('-i',$source,'-map','0','-c','copy',$out)
        $null = Assert-Media $out 'h264'
    }
    $videoOnly = Join-Path $fixture 'video-only.mp4'
    $audioOnly = Join-Path $fixture 'audio-only.m4a'
    $merged = Join-Path $fixture 'merged.mp4'
    Run-Media @('-i',$source,'-map','0:v:0','-an','-c','copy',$videoOnly)
    Run-Media @('-i',$source,'-map','0:a:0','-vn','-c','copy',$audioOnly)
    Run-Media @('-i',$videoOnly,'-i',$audioOnly,'-map','0:v:0','-map','1:a:0','-c','copy','-movflags','+faststart',$merged)
    $muxed = Assert-Media $merged 'h264'
    if (!($muxed.streams | Where-Object codec_name -EQ 'aac')) { throw 'Merged output lost its audio stream.' }
    $webm = Join-Path $fixture 'fallback.webm'
    Run-Media @('-i',$source,'-c:v','libvpx-vp9','-c:a','libopus',$webm)
    $null = Assert-Media $webm 'vp9'
    $fitted = Join-Path $fixture 'discord.mp4'
    Run-Media @('-i',$webm,'-vf','scale=-2:32','-c:v','libx264','-pix_fmt','yuv420p','-c:a','aac',$fitted)
    $null = Assert-Media $fitted 'h264'
    $webp = Join-Path $fixture 'discord.webp'
    Run-Media @('-i',$cover,'-vf','scale=32:-2','-c:v','libwebp','-quality','80',$webp)
    if (!(Test-Path -LiteralPath $webp)) { throw 'Discord image fitting failed.' }
    # Execute from the bundle directly; no installed FFmpeg or FFprobe is used.
    Write-Host 'PASS: fail-closed staging, pinned payload, seven audio formats, metadata/art, preview filters, remux, video/audio conversion and image fitting.'
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    if (!$resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
