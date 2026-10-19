# VM / Windows Sandbox ONLY. Installs and registers the product text service (x64 + x86),
# then (by default) enables it for the current user via InstallLayoutOrTip.
#
#   powershell -ExecutionPolicy Bypass -File register.ps1 [-PassPrintable] [-HklSubstitute]
#              [-NoUserEnable] [-Force]
#
# -PassPrintable  HKCU\...\Tip\EatPrintable = 0: idle digits/punctuation/Space pass through to the
#                 Persian layout instead of being inserted by the IME (default: eaten)
# -HklSubstitute  register the profile with hklSubstitute = 0x04090409 (PLAN 2.4 experiment)
# -DistDir        folder holding x64\pime_tip.dll and x86\pime_tip.dll (default: ..\)
param(
    [string]$DistDir = (Split-Path $PSScriptRoot -Parent),
    [switch]$PassPrintable,
    [switch]$HklSubstitute,
    [switch]$NoUserEnable,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-SafeMachine -Force:$Force
Assert-Admin

$x64 = Join-Path $DistDir "x64\$($Script:DllName)"
$x86 = Join-Path $DistDir "x86\$($Script:DllName)"
if (-not (Test-Path $x64)) { throw "Missing $x64 (run scripts\build.ps1 on the host first)" }

# Program Files: AppContainer (immersive) processes can read it there by default.
$dst64 = Join-Path $Script:InstallRoot 'x64'
$dst86 = Join-Path $Script:InstallRoot 'x86'
New-Item -ItemType Directory -Force $dst64, $dst86 | Out-Null
Copy-Item $x64 $dst64 -Force
if (Test-Path $x86) { Copy-Item $x86 $dst86 -Force }

# Runtime switches, read by the DLL at each activation.
New-Item -Path $Script:TipKey -Force | Out-Null
Set-ItemProperty $Script:TipKey -Name EatPrintable -Type DWord -Value ([int](-not $PassPrintable))
Set-ItemProperty $Script:AppKey -Name Disabled -Type DWord -Value 0

if ($HklSubstitute) { $env:PIME_HKL_SUBSTITUTE = '1' } else { Remove-Item Env:PIME_HKL_SUBSTITUTE -ErrorAction SilentlyContinue }

Invoke-Regsvr32 "$env:windir\System32\regsvr32.exe" (Join-Path $dst64 $Script:DllName)
Write-Host 'x64 registered'
if (Test-Path (Join-Path $dst86 $Script:DllName)) {
    Invoke-Regsvr32 "$env:windir\SysWOW64\regsvr32.exe" (Join-Path $dst86 $Script:DllName)
    Write-Host 'x86 registered'
}

if (-not $NoUserEnable) {
    if (-not (Set-TipEnabledForUser $true)) {
        Write-Warning 'Falling back to Set-WinUserLanguageList.'
        $list = Get-WinUserLanguageList
        $fa = $list | Where-Object LanguageTag -eq 'fa-IR'
        if (-not $fa) { $list.Add('fa-IR'); $fa = $list | Where-Object LanguageTag -eq 'fa-IR' }
        $fa.InputMethodTips.Add($Script:TipString)
        Set-WinUserLanguageList $list -Force
    }
}

& (Join-Path $PSScriptRoot 'test.ps1') -Check Registered
Write-Host ''
Write-Host 'Done. Switch to "Persian Phonetic" with Win+Space (restart apps if it does not show up).'
Write-Host 'Emergency: killswitch.ps1 -On (takes effect at next activation), or Win+Space to another keyboard.'
