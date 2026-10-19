# Read-only checks of the spike's registration state, plus the manual test checklist.
#   test.ps1 -Check Registered     after register.ps1
#   test.ps1 -Check Unregistered   after unregister.ps1 (asserts nothing is left)
#   test.ps1 -Checklist            print the manual typing checklist
param([ValidateSet('Registered', 'Unregistered', 'None')][string]$Check = 'None', [switch]$Checklist)
. (Join-Path $PSScriptRoot 'common.ps1')

$keys = [ordered]@{
    'CLSID (64-bit view)'          = "HKLM:\SOFTWARE\Classes\CLSID\$($Script:Clsid)\InprocServer32"
    'CLSID (32-bit view)'          = "HKLM:\SOFTWARE\WOW6432Node\Classes\CLSID\$($Script:Clsid)\InprocServer32"
    'TIP key'                      = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)"
    'TIP profile fa-IR'            = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)\LanguageProfile\0x00000429\$($Script:ProfileGuid)"
    'TIP categories'               = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)\Category"
    'TIP key (WOW6432Node)'        = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$($Script:Clsid)"
    'Per-user TIP state'           = "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$($Script:Clsid)"
}
$failed = 0
function Report([string]$name, [bool]$present, [bool]$wanted) {
    $mark = if ($present -eq $wanted) { 'ok  ' } else { $script:failed++; 'FAIL' }
    Write-Host ("[{0}] {1,-26} {2}" -f $mark, $name, $(if ($present) { 'present' } else { 'absent' }))
}

if ($Check -ne 'None') {
    $want = $Check -eq 'Registered'
    foreach ($k in $keys.Keys) {
        $present = Test-Path $keys[$k]
        # The WOW6432Node TIP copy and the HKCU state are informational only (behaviour differs
        # by Windows build; recording it is the point).
        if ($k -in 'TIP key (WOW6432Node)', 'Per-user TIP state') {
            Write-Host ("[info] {0,-26} {1}" -f $k, $(if ($present) { 'present' } else { 'absent' }))
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
    }
    $tips = (Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips }) -join ' '
    $enabled = $tips -match [regex]::Escape($Script:Clsid.Trim('{}'))
    Write-Host ("[info] {0,-26} {1}" -f 'User language list', $(if ($enabled) { 'contains the spike' } else { 'does not contain the spike' }))
    if ($Check -eq 'Unregistered' -and $enabled) { $failed++; Write-Host '[FAIL] user language list still references the spike' }
    if ($failed) { Write-Host "$failed check(s) failed"; exit 1 } else { Write-Host 'All checks passed.' }
}

if ($Checklist -or $Check -eq 'Registered') {
@'

Manual checklist (record results in docs/spike.md, section "VM results"):
 Switch to "Persian Phonetic (spike)" with Win+Space in each host first.
 1. Notepad: type s h -> underlined س then ش; Space -> "ش " committed.
 2. Type salam, Backspace twice -> preedit shrinks per Latin char; Escape -> preedit gone.
 3. Type mi, Shift+Space, x, Space -> می‌خ (ZWNJ inside). Idle Shift+Space twice -> ZWNJ added then removed.
 4. Type a, then Left arrow -> commits ا, caret moves left (commit-then-pass).
 5. Enter while composing: default commits only (no newline). Re-register with -EnterPasses:
    commits AND inserts the newline, in that order, in Notepad, WordPad, Edge (textarea + chat-like input).
 6. Idle digits/punctuation: default (pass-through under the fa-IR layout) -> note which characters
    appear. Then -EatPrintable -> must be ASCII 1 2 , . ; and with -HklSubstitute (no EatPrintable) -> note.
 7. Word, Edge (input, textarea, contenteditable), Settings search box, Start search: repeat 1-2.
    Immersive hosts (Settings/Start search) are expected to ignore an UNSIGNED TIP; repeat after sign.ps1.
 8. Password field (Edge login form, or Win+R runas prompt): TIP must not compose.
 9. Click elsewhere mid-composition -> text stays, no underline left, next key starts fresh.
10. killswitch.ps1 -On, switch away and back (or restart Notepad): typing produces Latin/Persian-layout
    chars only, nothing composes. killswitch.ps1 -Off restores.
11. unregister.ps1 -> every check reports absent; the profile is gone from Win+Space.
12. Event Viewer > Windows Logs > Application: no crashes of notepad/msedge/SearchApp naming pime_spike.dll.
'@ | Write-Host
}
