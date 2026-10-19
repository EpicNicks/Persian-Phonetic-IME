# VM / Windows Sandbox ONLY. Reverses register.ps1 and verifies nothing is left behind.
param([switch]$KeepFiles, [switch]$Force)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-SafeMachine -Force:$Force
Assert-Admin

# 1. Per-user: remove from the user's input methods first, while the profile still exists.
[void](Set-TipEnabledForUser $false)

# 2. Machine: DllUnregisterServer (categories, profile, TIP key, CLSID) for both views.
$dll64 = Join-Path $Script:InstallRoot 'x64\pime_spike.dll'
$dll86 = Join-Path $Script:InstallRoot 'x86\pime_spike.dll'
if (Test-Path $dll86) { Invoke-Regsvr32 "$env:windir\SysWOW64\regsvr32.exe" $dll86 -Unregister; Write-Host 'x86 unregistered' }
if (Test-Path $dll64) { Invoke-Regsvr32 "$env:windir\System32\regsvr32.exe" $dll64 -Unregister; Write-Host 'x64 unregistered' }

# 3. Files. A DLL still loaded in some process cannot be deleted: rename it aside instead and
#    let the next reboot (or Sandbox close) clean up.
if (-not $KeepFiles -and (Test-Path $Script:InstallRoot)) {
    try { Remove-Item $Script:InstallRoot -Recurse -Force }
    catch {
        Get-ChildItem $Script:InstallRoot -Recurse -Filter *.dll | ForEach-Object {
            Rename-Item $_.FullName ($_.Name + '.old-' + [guid]::NewGuid().ToString('N')) -ErrorAction SilentlyContinue
        }
        Write-Warning "Some files are in use; renamed aside. Reboot, then delete $($Script:InstallRoot)."
    }
}
Remove-Item $Script:SpikeKey -Recurse -Force -ErrorAction SilentlyContinue

& (Join-Path $PSScriptRoot 'test.ps1') -Check Unregistered
