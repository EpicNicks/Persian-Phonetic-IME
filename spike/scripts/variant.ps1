# Switch the spike's experimental behaviour without re-registering. Writes only
# HKCU\Software\PersianPhoneticIme\Spike (the spike's own key). The DLL reads it at activation:
# after switching, close and reopen the test app (or Win+Space away and back).
#
#   variant.ps1 -List
#   variant.ps1 -Name A3
#   variant.ps1 -Name C3 -EnterPasses        (extra flags are added on top of the preset)
param(
    [string]$Name,
    [switch]$List,
    [switch]$EnterPasses,
    [switch]$EatPrintable
)
. (Join-Path $PSScriptRoot 'common.ps1')

$Script:Variants = [ordered]@{
    '0'  = @{ d = 'baseline: composition, underline, Space commits then inserts " " via a one-shot composition'; v = @{} }
    'A1' = @{ d = 'RLM around preedit, stripped at commit'; v = @{ BidiMark = 'rlm' } }
    'A2' = @{ d = 'RLM around preedit, kept in committed text'; v = @{ BidiMark = 'rlm'; BidiKeep = $true } }
    'A3' = @{ d = 'RLE..PDF around preedit, stripped at commit'; v = @{ BidiMark = 'rle' } }
    'A4' = @{ d = 'RLE..PDF around preedit, kept'; v = @{ BidiMark = 'rle'; BidiKeep = $true } }
    'A5' = @{ d = 'RLI..PDI around preedit, stripped (isolates may render as boxes in old controls)'; v = @{ BidiMark = 'rli' } }
    'B1' = @{ d = 'Space stays in the composition until the next letter'; v = @{ SpaceInComposition = $true } }
    'B2' = @{ d = 'B1 + RLM kept'; v = @{ SpaceInComposition = $true; BidiMark = 'rlm'; BidiKeep = $true } }
    'C1' = @{ d = 'space after commit inserted as plain text (no one-shot composition)'; v = @{ PlainInsert = $true } }
    'C2' = @{ d = 'no underline display attribute'; v = @{ NoAttribute = $true } }
    'C3' = @{ d = 'NO composition: text inserted into the document and replaced per key (web-keyboard model)'; v = @{ NoComposition = $true } }
    'C4' = @{ d = 'C3 + RLM kept'; v = @{ NoComposition = $true; BidiMark = 'rlm'; BidiKeep = $true } }
}

if ($List -or -not $Name) {
    foreach ($k in $Script:Variants.Keys) { Write-Host ("{0,-3} {1}" -f $k, $Script:Variants[$k].d) }
    Write-Host ''
    Write-Host -NoNewline 'Current: '; Show-SpikeOptions
    return
}
if (-not $Script:Variants.Contains($Name)) { throw "Unknown variant '$Name' (use -List)" }
$values = $Script:Variants[$Name].v.Clone()
if ($EnterPasses) { $values.EnterPasses = $true }
if ($EatPrintable) { $values.EatPrintable = $true }
Set-SpikeOptions $values
Write-Host "Variant $Name : $($Script:Variants[$Name].d)"
Show-SpikeOptions
Write-Host 'Now close and reopen the test app (or Win+Space away and back) so the TIP re-activates.'
