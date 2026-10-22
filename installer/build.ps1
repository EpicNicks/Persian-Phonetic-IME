# Builds the Persian Phonetic IME MSI. Host-safe: compiles, stages and packages only. It never runs
# msiexec, regsvr32, pime-setup /register or InstallLayoutOrTip.
#
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 [-Version 0.1.0]
#       [-DllX64 <path> -DllX86 <path>] [-SkipTests] [-Validate]
#       [-CertThumbprint <sha1> [-TimestampUrl <url>] [-SignTool <path>]]
#
# Without -DllX64/-DllX86 the spike DLLs in spike\dist are used as PLACEHOLDER payloads: the MSI is
# then named *-PLACEHOLDER.msi, says so in its product name, and passes the spike's CLSID/profile
# GUIDs to pime-setup so the per-user enable step can be exercised in a VM.
# Output: installer\out\PersianPhoneticIme-<Version>-x64[-PLACEHOLDER].msi
[CmdletBinding()]
param(
    [string]$Version = '0.1.0',
    [string]$DllX64,
    [string]$DllX86,
    [switch]$SkipTests,
    [switch]$SkipRust,
    [switch]$Validate,
    [string]$CertThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [string]$SignTool,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Installer = $PSScriptRoot
$Root = Split-Path -Parent $Installer
if (-not $OutDir) { $OutDir = Join-Path $Installer 'out' }
$TargetDir = Join-Path $Root 'target-ws8'

function Invoke-Checked([string]$Exe, [string[]]$Arguments) {
    Write-Host "> $Exe $($Arguments -join ' ')"
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}

# --- version -------------------------------------------------------------------------------------
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$' -or [int]$Matches[1] -gt 255 -or [int]$Matches[2] -gt 255 -or [int]$Matches[3] -gt 65535) {
    throw "Version '$Version' must be major.minor.build (<= 255.255.65535); MSI ignores a fourth field for upgrades."
}

# --- GUIDs: single source of truth is crates/pime-ids/src/lib.rs ----------------------------
function Read-Guids([string]$Path) {
    $map = @{}
    foreach ($m in [regex]::Matches((Get-Content -Raw $Path), 'pub const (\w+): u128 = 0x([0-9a-fA-F_]+);')) {
        $hex = $m.Groups[2].Value.Replace('_', '').ToUpper()
        if ($hex.Length -ne 32) { throw "Bad GUID literal for $($m.Groups[1].Value) in $Path" }
        $map[$m.Groups[1].Value] = '{' + $hex.Substring(0, 8) + '-' + $hex.Substring(8, 4) + '-' + $hex.Substring(12, 4) + '-' + $hex.Substring(16, 4) + '-' + $hex.Substring(20) + '}'
    }
    return $map
}
$Guids = Read-Guids (Join-Path $Root 'crates\pime-ids\src\lib.rs')
foreach ($k in 'CLSID_TEXT_SERVICE', 'GUID_PROFILE_FA_PHONETIC', 'MSI_UPGRADE_CODE', 'ACTIVE_SETUP_ID') {
    if (-not $Guids.ContainsKey($k)) { throw "guids.rs has no $k" }
}
$TipClsid = $Guids.CLSID_TEXT_SERVICE
$TipProfile = $Guids.GUID_PROFILE_FA_PHONETIC
$GuidArgs = ''

# --- payload DLLs ---------------------------------------------------------------------------------
$Placeholder = -not $DllX64 -and -not $DllX86
if ($Placeholder) {
    $DllX64 = Join-Path $Root 'spike\dist\x64\pime_spike.dll'
    $DllX86 = Join-Path $Root 'spike\dist\x86\pime_spike.dll'
    $spikeGuids = Join-Path $Root 'spike\src\guids.rs'
    $spike = @{}
    foreach ($m in [regex]::Matches((Get-Content -Raw $spikeGuids), 'pub const (\w+): GUID = GUID::from_u128\(0x([0-9a-fA-F_]+)\);')) {
        $hex = $m.Groups[2].Value.Replace('_', '').ToUpper()
        $spike[$m.Groups[1].Value] = '{' + $hex.Substring(0, 8) + '-' + $hex.Substring(8, 4) + '-' + $hex.Substring(12, 4) + '-' + $hex.Substring(16, 4) + '-' + $hex.Substring(20) + '}'
    }
    $TipClsid = $spike.CLSID_TEXT_SERVICE
    $TipProfile = $spike.GUID_PROFILE
    if (-not $TipClsid -or -not $TipProfile) { throw "Could not read spike GUIDs from $spikeGuids" }
    $GuidArgs = "--clsid $TipClsid --profile $TipProfile"
    Write-Warning "No -DllX64/-DllX86 given: building a PLACEHOLDER MSI from the spike DLLs (VM testing of installer mechanics only)."
} elseif (-not $DllX64 -or -not $DllX86) {
    throw 'Pass both -DllX64 and -DllX86 (or neither, for a placeholder build).'
}

function Get-PeMachine([string]$Path) {
    $b = [IO.File]::ReadAllBytes($Path)
    if ($b.Length -lt 0x40 -or $b[0] -ne 0x4D -or $b[1] -ne 0x5A) { throw "$Path is not a PE file" }
    $pe = [BitConverter]::ToInt32($b, 0x3C)
    if ([BitConverter]::ToUInt32($b, $pe) -ne 0x4550) { throw "$Path is not a PE file" }
    $chars = [BitConverter]::ToUInt16($b, $pe + 22)
    if (($chars -band 0x2000) -eq 0) { throw "$Path is not a DLL" }
    return [BitConverter]::ToUInt16($b, $pe + 4)
}
foreach ($p in $DllX64, $DllX86) { if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { throw "Missing payload $p" } }
if ((Get-PeMachine $DllX64) -ne 0x8664) { throw "$DllX64 is not an x64 DLL" }
if ((Get-PeMachine $DllX86) -ne 0x014C) { throw "$DllX86 is not an x86 DLL" }

# --- Rust: pime-setup (x64 + x86), pime-settings (x64), static CRT + CFG -------------------------
$env:CARGO_TARGET_DIR = $TargetDir
$flags = '-C target-feature=+crt-static -C control-flow-guard'
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $flags
$env:CARGO_TARGET_I686_PC_WINDOWS_MSVC_RUSTFLAGS = $flags
Push-Location $Root
try {
    if (-not $SkipRust) {
        if (-not $SkipTests) { Invoke-Checked 'cargo' @('test', '-p', 'pime-setup') }
        Invoke-Checked 'cargo' @('build', '--release', '-p', 'pime-setup', '--target', 'x86_64-pc-windows-msvc')
        Invoke-Checked 'cargo' @('build', '--release', '-p', 'pime-setup', '--target', 'i686-pc-windows-msvc')
        Invoke-Checked 'cargo' @('build', '--release', '-p', 'pime-settings', '--target', 'x86_64-pc-windows-msvc')
    }
} finally {
    Pop-Location
}
$SetupX64 = Join-Path $TargetDir 'x86_64-pc-windows-msvc\release\pime-setup.exe'
$SetupX86 = Join-Path $TargetDir 'i686-pc-windows-msvc\release\pime-setup.exe'
$SettingsExe = Join-Path $TargetDir 'x86_64-pc-windows-msvc\release\pime-settings.exe'
foreach ($p in $SetupX64, $SetupX86, $SettingsExe) { if (-not (Test-Path $p)) { throw "Missing build output $p" } }

# --- dictionary (AC-G): dict.bin + notices, built into the stage folder below ---------------------
$DictSrc = Join-Path $Root 'data\dict'
$DictTsvs = @('fa-wordfreq.tsv', 'wordfreq-fa.tsv', 'extra.tsv') | ForEach-Object { Join-Path $DictSrc $_ } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
if (-not $DictTsvs) { throw "No dictionary source TSV in $DictSrc" }
$NoticeSrc = Join-Path $DictSrc 'NOTICE.txt'
$LicenseSrc = Join-Path $DictSrc 'CC-BY-SA-4.0.txt'
foreach ($p in $NoticeSrc, $LicenseSrc) { if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { throw "Missing $p (required third-party notice)" } }

# --- stage ----------------------------------------------------------------------------------------
$Stage = Join-Path $OutDir "stage\$Version"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force $Stage | Out-Null
Copy-Item $DllX64 (Join-Path $Stage "PersianPhoneticIme.$Version.x64.dll")
Copy-Item $DllX86 (Join-Path $Stage "PersianPhoneticIme.$Version.x86.dll")
Copy-Item $SetupX64 (Join-Path $Stage 'pime-setup.x64.exe')
Copy-Item $SetupX86 (Join-Path $Stage 'pime-setup.x86.exe')
Copy-Item $SettingsExe (Join-Path $Stage 'pime-settings.exe')
$DictBin = Join-Path $Stage 'dict.bin'
Push-Location $Root
try {
    $dictArgs = @('run', '-p', 'xtask', '--release', '--', 'build-dict')
    foreach ($t in $DictTsvs) { $dictArgs += @('--input', $t) }
    $dictArgs += @('--out', $DictBin)
    Invoke-Checked 'cargo' $dictArgs
} finally {
    Pop-Location
}
if (-not (Test-Path -LiteralPath $DictBin -PathType Leaf) -or (Get-Item -LiteralPath $DictBin).Length -eq 0) {
    throw "build-dict did not produce a non-empty $DictBin"
}
Copy-Item $NoticeSrc (Join-Path $Stage 'THIRD-PARTY-NOTICES.txt')
New-Item -ItemType Directory -Force (Join-Path $Stage 'licenses') | Out-Null
Copy-Item $LicenseSrc (Join-Path $Stage 'licenses\CC-BY-SA-4.0.txt')
Copy-Item (Join-Path $Root 'LICENSE') (Join-Path $Stage 'licenses\GPL-3.0.txt')
if ($Placeholder) {
    @"
PLACEHOLDER BUILD - NOT A RELEASE.
The TIP DLLs in this folder are the WS1 spike (spike\dist), renamed to the product's versioned
file names. They register the spike CLSID $TipClsid, not the product CLSID.
Use this MSI only in a VM or Windows Sandbox to test the installer mechanics.
"@ | Set-Content -Encoding ASCII (Join-Path $Stage 'PLACEHOLDER.txt')
}

# --- optional signing (no secrets here: the certificate comes from the user's cert store) --------
function Find-SignTool {
    if ($SignTool) { return $SignTool }
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $kits = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($kits) { return $kits.FullName }
    throw 'signtool.exe not found; pass -SignTool'
}
function Invoke-Sign([string[]]$Files) {
    if (-not $CertThumbprint) { return }
    $tool = Find-SignTool
    Invoke-Checked $tool (@('sign', '/sha1', $CertThumbprint, '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256') + $Files)
}
Invoke-Sign (Get-ChildItem $Stage -Include *.dll, *.exe -Recurse | ForEach-Object FullName)

# --- WiX ------------------------------------------------------------------------------------------
$wix = Get-Command wix -ErrorAction SilentlyContinue
if (-not $wix) {
    $candidate = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
    if (Test-Path $candidate) { $wix = Get-Item $candidate } else {
        throw 'WiX v5 not found. Install: dotnet tool install --global wix --version 5.0.2; wix extension add -g WixToolset.Util.wixext/5.0.2'
    }
}
$wixExe = if ($wix -is [System.Management.Automation.CommandInfo]) { $wix.Source } else { $wix.FullName }
$wixVersion = & $wixExe --version
if ($wixVersion -notmatch '^5\.') { throw "WiX v5 required, found $wixVersion" }
$ext = & $wixExe extension list -g
if (-not ($ext -match 'WixToolset\.Util\.wixext 5\.')) {
    throw 'WiX Util extension missing. Install: wix extension add -g WixToolset.Util.wixext/5.0.2'
}

$suffix = if ($Placeholder) { '-PLACEHOLDER' } else { '' }
$Msi = Join-Path $OutDir "PersianPhoneticIme-$Version-x64$suffix.msi"
$wixArgs = @(
    'build', (Join-Path $Installer 'Package.wxs'),
    '-arch', 'x64',
    '-ext', 'WixToolset.Util.wixext',
    '-d', "Version=$Version",
    '-d', "StageDir=$Stage",
    '-d', "TipClsid=$TipClsid",
    '-d', "TipProfile=$TipProfile",
    '-d', "UpgradeCode=$($Guids.MSI_UPGRADE_CODE)",
    '-d', "ActiveSetupId=$($Guids.ACTIVE_SETUP_ID)",
    '-d', "Placeholder=$([int]$Placeholder)",
    '-o', $Msi
)
if ($GuidArgs) { $wixArgs += @('-d', "GuidArgs=$GuidArgs") }
Invoke-Checked $wixExe $wixArgs

if ($Validate) {
    # ICE validation opens the package read-only; it does not install anything.
    # ICE43/ICE57 are suppressed: they flag file-keyed shortcuts in ProgramMenuFolder because a
    # package *could* be per-user; this one is always per-machine (Scope=perMachine, ALLUSERS=1),
    # so the shortcuts land in the all-users Start menu and need no HKCU key path.
    Invoke-Checked $wixExe @('msi', 'validate', '-sice', 'ICE43', '-sice', 'ICE57', $Msi)
}

Invoke-Sign @($Msi)

Write-Host ''
Write-Host "Built $Msi"
if ($Placeholder) { Write-Warning 'PLACEHOLDER MSI (spike DLLs). Test only in a VM or Windows Sandbox.' }
Write-Host 'Nothing was installed or registered. Test the MSI in a VM/Sandbox (installer\README.md).'
