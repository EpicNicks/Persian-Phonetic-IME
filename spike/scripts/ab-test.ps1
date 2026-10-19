# Guided A/B run of the bidi/composition variants in classic Notepad (run inside the sandbox
# after register.ps1). For each variant it switches options, starts a fresh Notepad, tells you
# what to type and records what you saw in ab-results.txt on the Desktop.
#   ab-test.ps1                 all variants
#   ab-test.ps1 -Only A3,B1,C3  a subset
param([string[]]$Only, [switch]$Force)
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-SafeMachine -Force:$Force   # it closes every running Notepad
$variantScript = Join-Path $PSScriptRoot 'variant.ps1'
$out = Join-Path ([Environment]::GetFolderPath('Desktop')) 'ab-results.txt'
$names = @('0', 'A1', 'A2', 'A3', 'A4', 'A5', 'B1', 'B2', 'C1', 'C2', 'C3', 'C4')
if ($Only) { $Only = $Only -split ','; $names = $names | Where-Object { $_ -in $Only } }

"# spike A/B $(Get-Date -Format s) $([Environment]::OSVersion.VersionString)" | Add-Content $out
foreach ($n in $names) {
    & $variantScript -Name $n | Out-Host
    Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Process notepad.exe
    Write-Host @'

In the new Notepad: Win+Space -> "Persian Phonetic (spike)", then type slowly:
    salam<Space>dust<Space>
Watch WHERE the underlined word is drawn while typing, and whether it jumps on Space.
Then: Backspace x3, Escape, and Shift+Space between "mi" and "x" (mi<Shift+Space>x<Space>).
Finally Ctrl+A, Ctrl+C and paste into the sandbox's Edge address bar or a new Notepad with
"Format > Right-to-left reading order" to see whether bidi marks ended up in the text.
'@
    $where = Read-Host "[$n] Preedit position while typing (ok = where it ends up / right = far right / other)"
    $jump  = Read-Host "[$n] Jump on Space? (y/n)"
    $edit  = Read-Host "[$n] Backspace/Escape/ZWNJ behaved? (y/n + note)"
    $notes = Read-Host "[$n] Other notes (stray marks, boxes, caret position)"
    "{0}`tposition={1}`tjump={2}`tediting={3}`t{4}" -f $n, $where, $jump, $edit, $notes | Add-Content $out
}
& $variantScript -Name '0' | Out-Null
Write-Host "Results appended to $out (options reset to baseline)."
