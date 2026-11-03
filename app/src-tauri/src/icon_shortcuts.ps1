$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$iconPath = $env:ZOLTRAAK_ICON_PATH
$targetPath = [IO.Path]::GetFullPath($env:ZOLTRAAK_ICON_TARGET)
$shell = New-Object -ComObject WScript.Shell
$updated = 0
$failures = 0
$references = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ZoltraakIconShell {
  [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
  public static extern void SHChangeNotify(uint eventId, uint flags, string item, IntPtr other);
}
'@
$folders = @('DesktopDirectory', 'CommonDesktopDirectory', 'Programs', 'CommonPrograms')
foreach ($folder in $folders) {
  $directory = [Environment]::GetFolderPath([Environment+SpecialFolder]::$folder)
  if (!$directory -or !(Test-Path -LiteralPath $directory)) { continue }
  $recursive = $folder -eq 'Programs' -or $folder -eq 'CommonPrograms'
  $links = Get-ChildItem -LiteralPath $directory -Filter '*.lnk' -File -Recurse:$recursive -ErrorAction SilentlyContinue
  foreach ($linkFile in $links) {
    try {
      $link = $shell.CreateShortcut($linkFile.FullName)
      $oldIcon = ($link.IconLocation -replace ',\s*-?\d+$', '').Trim('"')
      if ($oldIcon -and [string]::Equals([IO.Path]::GetDirectoryName($oldIcon), [IO.Path]::GetDirectoryName($iconPath), [StringComparison]::OrdinalIgnoreCase)) { [void]$references.Add($oldIcon) }
      if (!$link.TargetPath -or ![IO.Path]::GetFullPath($link.TargetPath).Equals($targetPath, [StringComparison]::OrdinalIgnoreCase)) { continue }
      if ($link.IconLocation -ne "$iconPath,0") {
        $link.IconLocation = "$iconPath,0"
        $link.Save()
      }
      [void]$references.Add($iconPath)
      [ZoltraakIconShell]::SHChangeNotify(0x2000, 0x1005, $linkFile.FullName, [IntPtr]::Zero)
      $updated++
    } catch { $failures++ }
  }
}
@{ updated = $updated; failures = $failures; references = @($references) } | ConvertTo-Json -Compress
