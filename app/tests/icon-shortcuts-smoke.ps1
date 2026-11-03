$ErrorActionPreference = 'Stop'
$fixtureRoot = Join-Path $env:TEMP ('zoltraak-icon-qa-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$shell = New-Object -ComObject WScript.Shell
$fixtureTarget = Join-Path $fixtureRoot 'Zoltraak.exe'
$fixtureIcon = Join-Path $fixtureRoot 'theme.ico'
[IO.File]::WriteAllText($fixtureTarget, '')
[IO.File]::WriteAllText($fixtureIcon, '')
foreach ($name in @('Zoltraak', 'Other')) {
  $shortcut = $shell.CreateShortcut((Join-Path $fixtureRoot ($name + '.lnk')))
  $shortcut.TargetPath = $(if ($name -eq 'Zoltraak') { $fixtureTarget } else { Join-Path $fixtureRoot 'Other.exe' })
  $shortcut.Arguments = '--silent'
  $shortcut.Description = 'Keep this description'
  $shortcut.WorkingDirectory = $fixtureRoot
  $shortcut.Save()
}
$scriptPath = Join-Path $PSScriptRoot '../src-tauri/src/icon_shortcuts.ps1'
$fixtureScript = [IO.File]::ReadAllText($scriptPath).Replace("@('DesktopDirectory', 'CommonDesktopDirectory', 'Programs', 'CommonPrograms')", "@('DesktopDirectory')").Replace('[Environment]::GetFolderPath([Environment+SpecialFolder]::$folder)', '$env:ZOLTRAAK_ICON_FIXTURE')
$env:ZOLTRAAK_ICON_FIXTURE = $fixtureRoot
$env:ZOLTRAAK_ICON_PATH = $fixtureIcon
$env:ZOLTRAAK_ICON_TARGET = $fixtureTarget
$originalOtherIcon = $shell.CreateShortcut((Join-Path $fixtureRoot 'Other.lnk')).IconLocation
$result = (& ([scriptblock]::Create($fixtureScript))) | ConvertFrom-Json
if ($result.updated -ne 1 -or $result.failures -ne 0) { throw 'Expected exactly one matching shortcut update' }
$changed = $shell.CreateShortcut((Join-Path $fixtureRoot 'Zoltraak.lnk'))
if ($changed.IconLocation -ne "$fixtureIcon,0" -or $changed.Arguments -ne '--silent' -or $changed.Description -ne 'Keep this description' -or $changed.WorkingDirectory -ne $fixtureRoot) { throw 'Shortcut data was not preserved' }
$other = $shell.CreateShortcut((Join-Path $fixtureRoot 'Other.lnk'))
if ($other.IconLocation -ne $originalOtherIcon) { throw 'Unrelated shortcut was modified' }
'Windows shortcut checks passed: matching target updated, unrelated target untouched, launch arguments and metadata preserved.'
