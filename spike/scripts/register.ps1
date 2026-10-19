# VM / Windows Sandbox ONLY. Installs and registers the spike text service (x64 + x86),
# then (by default) enables it for the current user via InstallLayoutOrTip.
#
#   powershell -ExecutionPolicy Bypass -File register.ps1 [-HklSubstitute] [-EnterPasses]
#              [-EatPrintable] [-NoUserEnable] [-Force]
#
# -HklSubstitute  register the profile with hklSubstitute = 0x04090409 (PLAN 2.4 experiment)
# -EnterPasses    Enter commits and is passed to the app (TF_ES_SYNC path)
# -EatPrintable   idle digits/punctuation are eaten and inserted from the US table
# Bidi/layout experiments (docs/spike.md section 8; switch later without re-registering via variant.ps1):
# -BidiMark none|rlm|rle|rli   wrap the displayed composition in bidi controls
# -BidiKeep             keep those controls in committed text (default: stripped at commit)
# -SpaceInComposition   Space keeps "word + space" composing until the next letter
# -PlainInsert          insert the space after a commit as plain text (no one-shot composition)
# -NoAttribute          no underline display attribute
# -NoComposition        no TSF composition at all: insert text and keep replacing it
# -DistDir        folder holding x64\pime_spike.dll and x86\pime_spike.dll (default: ..\)
param(
    [string]$DistDir = (Split-Path $PSScriptRoot -Parent),
    [switch]$HklSubstitute,
    [switch]$EnterPasses,
    [switch]$EatPrintable,
    [ValidateSet('none', 'rlm', 'rle', 'rli')][string]$BidiMark = 'none',
    [switch]$BidiKeep,
    [switch]$SpaceInComposition,
    [switch]$PlainInsert,
    [switch]$NoAttribute,
    [switch]$NoComposition,
    [switch]$NoUserEnable,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-SafeMachine -Force:$Force
Assert-Admin

$x64 = Join-Path $DistDir 'x64\pime_spike.dll'
$x86 = Join-Path $DistDir 'x86\pime_spike.dll'
if (-not (Test-Path $x64)) { throw "Missing $x64 (run build.ps1 on the host and copy dist\ over)" }

# Copy into Program Files: AppContainer (immersive) processes can read it there by default.
$dst64 = Join-Path $Script:InstallRoot 'x64'
$dst86 = Join-Path $Script:InstallRoot 'x86'
New-Item -ItemType Directory -Force $dst64, $dst86 | Out-Null
# Fails if a running app still has the old DLL loaded: close those apps first, or switch
# behaviour with variant.ps1, which needs no re-registration.
Copy-Item $x64 $dst64 -Force
if (Test-Path $x86) { Copy-Item $x86 $dst86 -Force }

# Spike behaviour switches, read by the DLL on each activation.
Set-SpikeOptions @{
    EnterPasses = [bool]$EnterPasses; EatPrintable = [bool]$EatPrintable; BidiMark = $BidiMark
    BidiKeep = [bool]$BidiKeep; SpaceInComposition = [bool]$SpaceInComposition
    PlainInsert = [bool]$PlainInsert; NoAttribute = [bool]$NoAttribute; NoComposition = [bool]$NoComposition
}
Set-ItemProperty $Script:AppKey -Name Disabled -Type DWord -Value 0

if ($HklSubstitute) { $env:PIME_SPIKE_HKL_SUBSTITUTE = '1' } else { Remove-Item Env:PIME_SPIKE_HKL_SUBSTITUTE -ErrorAction SilentlyContinue }

Invoke-Regsvr32 "$env:windir\System32\regsvr32.exe" (Join-Path $dst64 'pime_spike.dll')
Write-Host 'x64 registered'
if (Test-Path (Join-Path $dst86 'pime_spike.dll')) {
    Invoke-Regsvr32 "$env:windir\SysWOW64\regsvr32.exe" (Join-Path $dst86 'pime_spike.dll')
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
Write-Host 'Done. Sign out/in (or restart apps) if the profile does not show up. Switch with Win+Space.'
Write-Host 'Emergency: run killswitch.ps1 -On (takes effect at next activation), or Win+Space to another keyboard.'
