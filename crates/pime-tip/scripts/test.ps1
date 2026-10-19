# Read-only checks of the TIP's registration state, plus the manual checklist.
#   test.ps1 -Check Registered     after register.ps1
#   test.ps1 -Check Unregistered   after unregister.ps1 (asserts nothing is left)
#   test.ps1 -Checklist            print the manual checklist (same as crates/pime-tip/README.md)
param([ValidateSet('Registered', 'Unregistered', 'None')][string]$Check = 'None', [switch]$Checklist)
. (Join-Path $PSScriptRoot 'common.ps1')

$keys = [ordered]@{
    'CLSID (64-bit view)'   = "HKLM:\SOFTWARE\Classes\CLSID\$($Script:Clsid)\InprocServer32"
    'CLSID (32-bit view)'   = "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\$($Script:Clsid)\InprocServer32"
    'TIP key'               = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)"
    'TIP profile fa-IR'     = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)\LanguageProfile\0x00000429\$($Script:ProfileGuid)"
    'TIP categories'        = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)\Category"
    'TIP key (WOW6432Node)' = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$($Script:Clsid)"
    'Per-user TIP state'    = "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)"
}
$failed = 0
function Report([string]$name, [bool]$present, [bool]$wanted) {
    $mark = if ($present -eq $wanted) { 'ok  ' } else { $script:failed++; 'FAIL' }
    Write-Host ("[{0}] {1,-24} {2}" -f $mark, $name, $(if ($present) { 'present' } else { 'absent' }))
}

if ($Check -ne 'None') {
    $want = $Check -eq 'Registered'
    foreach ($k in $keys.Keys) {
        $present = Test-Path $keys[$k]
        if ($k -in 'TIP key (WOW6432Node)', 'Per-user TIP state') {
            Write-Host ("[info] {0,-24} {1}" -f $k, $(if ($present) { 'present' } else { 'absent' }))
            continue
        }
        if (-not $want -or $k -ne 'CLSID (32-bit view)' -or (Test-Path (Join-Path $Script:InstallRoot 'x86'))) {
            Report $k $present $want
        }
    }
    if ($want -and (Test-Path $keys['TIP categories'])) {
        Write-Host 'Registered categories:'
        Get-ChildItem $keys['TIP categories'] -Recurse | Where-Object { $_.PSChildName -match '^\{' } |
            ForEach-Object { '    ' + $_.Name.Split('\')[-2] + ' / ' + $_.PSChildName } | Sort-Object -Unique
        $inproc = (Get-ItemProperty $keys['CLSID (64-bit view)'] -ErrorAction SilentlyContinue)
        Write-Host "InprocServer32 (x64): $($inproc.'(default)')  ThreadingModel=$($inproc.ThreadingModel)"
        $tip = Get-ItemProperty $Script:TipKey -ErrorAction SilentlyContinue
        Write-Host "EatPrintable=$($tip.EatPrintable)"
    }
    $tips = (Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips }) -join ' '
    $enabled = $tips -match [regex]::Escape($Script:Clsid.Trim('{}'))
    Write-Host ("[info] {0,-24} {1}" -f 'User language list', $(if ($enabled) { 'contains Persian Phonetic' } else { 'does not contain Persian Phonetic' }))
    if ($Check -eq 'Unregistered' -and $enabled) { $failed++; Write-Host '[FAIL] user language list still references the TIP' }
    if ($failed) { Write-Host "$failed check(s) failed"; exit 1 } else { Write-Host 'All checks passed.' }
}

if ($Checklist -or $Check -eq 'Registered') {
@'

Manual checklist (Notepad first, then Edge textarea / contenteditable). Switch to
"Persian Phonetic" with Win+Space. Full version: crates\pime-tip\README.md.
 1. s -> underlined س and a numbered list (س ص ث) under it; h -> ش, list closes. Space -> "ش ".
 2. s, Tab / Down / Up move the highlight (preedit follows); 1-9 pick and commit; mouse click picks.
 3. salam, Backspace twice -> preedit shrinks per Latin letter; Escape closes the list, Escape again
    cancels the word. Enter commits only (no newline).
 4. bc -> "ب" + grey dotted "c" (pending); h -> بچ.
 5. mi, Shift+Space, xam, Space -> می‌خم (ZWNJ inside). Idle Shift+Space twice -> ZWNJ then gone.
 6. Idle 1 2 , ? -> ۱ ۲ ، ؟ (default). config.ps1 -Good, click away and back -> ASCII digits and the
    list opens ABOVE the word.
 7. config.ps1 -Broken, click away and back -> language-bar button shows the warning badge/tooltip;
    typing still works with defaults. config.ps1 -Remove -> badge clears.
 8. Click the "ف" indicator (or its menu) -> "A": typing gives Latin letters (US layout), nothing
    composes. Back to "ف".
 9. ba then Left arrow -> commits با, caret moves. Click elsewhere mid-word -> text stays, no
    underline, next key starts fresh. Ctrl+A mid-word -> commits, then selects all.
10. Password field (Edge login form) -> nothing composes.
11. killswitch.ps1 -On, switch keyboard away and back -> nothing composes. -Off restores.
12. unregister.ps1 -> every check reports absent.
13. Event Viewer > Application: no crashes naming pime_tip.dll.
'@ | Write-Host
}
