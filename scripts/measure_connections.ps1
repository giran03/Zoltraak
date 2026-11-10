# Measures how many TCP sockets a segmented download really opens, and how
# fast it goes, by running the CLI against a real URL for a fixed time.
#
# A download that asks for N connections should hold N sockets to the server.
# Fewer means the requests are being multiplexed onto shared connections
# (HTTP/2), which loses most of the benefit of segmenting.
#
# Usage:
#   ./scripts/measure_connections.ps1 -Exe engine/target/debug/zoltraak.exe `
#       -Url https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.6.tar.xz -Connections 8
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Url,
    [int] $Connections = 8,
    [int] $Seconds = 20
)

$ErrorActionPreference = 'Stop'
$out = Join-Path ([System.IO.Path]::GetTempPath()) ("zoltraak-measure-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $out | Out-Null

$proc = Start-Process -FilePath $Exe -PassThru -NoNewWindow `
    -ArgumentList @('get', $Url, '--output', $out, '--connections', $Connections) `
    -RedirectStandardError (Join-Path $out 'stderr.log') -RedirectStandardOutput (Join-Path $out 'stdout.log')

$start = Get-Date
$maxSockets = 0
$samples = @()
while (-not $proc.HasExited -and ((Get-Date) - $start).TotalSeconds -lt $Seconds) {
    Start-Sleep -Milliseconds 500
    $sockets = @(Get-NetTCPConnection -OwningProcess $proc.Id -State Established -ErrorAction SilentlyContinue |
        Where-Object { $_.RemotePort -in 80, 443 }).Count
    $samples += $sockets
    if ($sockets -gt $maxSockets) { $maxSockets = $sockets }
}
$elapsed = ((Get-Date) - $start).TotalSeconds
$finished = $proc.HasExited
if (-not $finished) { Stop-Process -Id $proc.Id -Force; Start-Sleep -Milliseconds 300 }

# A finished download has its full file; a stopped one has a resume sidecar
# that records how much was written.
$bytes = 0
$sidecar = Get-ChildItem $out -Filter '*.zoltraak' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($sidecar) {
    $state = Get-Content $sidecar.FullName -Raw | ConvertFrom-Json
    $bytes = ($state.chunks | Measure-Object -Property downloaded -Sum).Sum
} elseif ($finished) {
    $file = Get-ChildItem $out -File | Where-Object { $_.Extension -notin '.log' } | Select-Object -First 1
    if ($file) { $bytes = $file.Length }
}

$busy = $samples | Select-Object -Skip 2
[pscustomobject]@{
    RequestedConnections = $Connections
    MaxSockets           = $maxSockets
    MedianSockets        = if ($busy) { ($busy | Sort-Object)[[int]($busy.Count / 2)] } else { 0 }
    Seconds              = [math]::Round($elapsed, 1)
    Finished             = $finished
    MiB                  = [math]::Round($bytes / 1MB, 1)
    MiBPerSecond         = [math]::Round($bytes / 1MB / $elapsed, 2)
} | Format-List

Remove-Item -Recurse -Force $out
