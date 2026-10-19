# Write test configs to %APPDATA%\PersianPhoneticIme\config.toml (per-user file only; safe anywhere,
# but meant for the sandbox). The TIP picks the change up on the next focus change.
#   config.ps1 -Good      a valid config (ASCII digits, candidate list above the word)
#   config.ps1 -Broken    a file with a bad value and a syntax error (expect the warning badge)
#   config.ps1 -BadValue  one bad value only (that option skipped, everything else applied)
#   config.ps1 -Remove    delete it (built-in defaults)
param([switch]$Good, [switch]$Broken, [switch]$BadValue, [switch]$Remove)
. (Join-Path $PSScriptRoot 'common.ps1')
$file = Join-Path $Script:ConfigDir 'config.toml'
New-Item -ItemType Directory -Force $Script:ConfigDir | Out-Null
if ($Remove) { Remove-Item $file -ErrorAction SilentlyContinue; Write-Host "Removed $file"; return }
$text = if ($Good) {
@'
[options]
digits = "ascii"
candidate_position = "above"
'@
} elseif ($BadValue) {
@'
[options]
gh = "kaf"
digits = "ascii"
'@
} elseif ($Broken) {
@'
[options]
digits = "ascii
gh = "qaf"
'@
} else { throw 'Pass one of -Good, -Broken, -BadValue, -Remove' }
# UTF-8 without BOM (the TIP accepts a BOM too).
[IO.File]::WriteAllText($file, $text, (New-Object Text.UTF8Encoding $false))
Write-Host "Wrote $file. Click into another window and back to reload."
