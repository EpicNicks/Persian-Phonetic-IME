# Shared helpers for the pime-tip sandbox scripts. Dot-source, do not run.
# Everything that registers anything refuses to run outside a VM / Windows Sandbox unless -Force.
# GUIDs: crates/pime-ids/src/lib.rs (frozen). NOT the spike's GUIDs.

$Script:Clsid       = '{49B3A09B-D707-4675-B4EB-62B17CCB6D73}'
$Script:ProfileGuid = '{C1074472-386F-4D95-AB2D-887EB94BD96D}'
$Script:LangHex     = '0429'
$Script:TipString   = "$($Script:LangHex):$($Script:Clsid)$($Script:ProfileGuid)"
$Script:TipStringHex = "0x$($Script:TipString)"
$Script:DllName     = 'pime_tip.dll'
$Script:InstallRoot = Join-Path $env:ProgramFiles 'PersianPhoneticImeDev'
$Script:AppKey      = 'HKCU:\Software\PersianPhoneticIme'
$Script:TipKey      = 'HKCU:\Software\PersianPhoneticIme\Tip'
$Script:ConfigDir   = Join-Path $env:APPDATA 'PersianPhoneticIme'

function Test-IsVirtualMachine {
    if ($env:USERNAME -eq 'WDAGUtilityAccount') { return $true }   # Windows Sandbox
    $cs = Get-CimInstance Win32_ComputerSystem
    $bios = Get-CimInstance Win32_BIOS
    $hints = @($cs.Manufacturer, $cs.Model, $bios.Manufacturer, $bios.SMBIOSBIOSVersion) -join ' '
    return $hints -match 'Virtual|VMware|VirtualBox|QEMU|KVM|Xen|Hyper-V|Parallels'
}

function Assert-SafeMachine([switch]$Force) {
    if (Test-IsVirtualMachine) { return }
    if ($Force) {
        Write-Warning 'Not a VM, continuing because -Force was given. Keep another keyboard (Win+Space) ready.'
        return
    }
    throw 'This does not look like a VM or Windows Sandbox. A broken text service breaks typing in every app. Re-run inside a VM, or pass -Force if you really mean it.'
}

function Assert-Admin {
    $id = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $id.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this from an elevated PowerShell (registration writes HKLM).'
    }
}

function Invoke-Regsvr32([string]$Exe, [string]$Dll, [switch]$Unregister) {
    $argList = @('/s')
    if ($Unregister) { $argList += '/u' }
    $argList += "`"$Dll`""
    $p = Start-Process -FilePath $Exe -ArgumentList $argList -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        throw "$Exe $($argList -join ' ') failed with exit code $($p.ExitCode)"
    }
}

function Get-InputDll {
    if (-not ('PimeTip.InputDll' -as [type])) {
        Add-Type -Namespace PimeTip -Name InputDll -MemberDefinition @'
[DllImport("input.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool InstallLayoutOrTip(string psz, uint dwFlags);
'@
    }
    return [PimeTip.InputDll]
}

# ILOT_UNINSTALL = 0x1
function Set-TipEnabledForUser([bool]$Enabled) {
    $flags = if ($Enabled) { 0 } else { 1 }
    foreach ($s in @($Script:TipString, $Script:TipStringHex)) {
        if ((Get-InputDll)::InstallLayoutOrTip($s, [uint32]$flags)) {
            Write-Host "InstallLayoutOrTip('$s', $flags) = TRUE"
            return $true
        }
        Write-Warning "InstallLayoutOrTip('$s', $flags) returned FALSE"
    }
    return $false
}
