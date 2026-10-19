# Shared helpers for the WS1 spike scripts. Dot-source, do not run.
# Everything that registers anything refuses to run outside a VM / Windows Sandbox unless -Force.

$Script:Clsid     = '{674850A2-4C3B-46FA-9B17-3CA5465FFA18}'
$Script:ProfileGuid   = '{DA9A94B1-BE61-4FF6-A706-7FEAF1E62A33}'
$Script:LangHex   = '0429'
$Script:TipString = "$($Script:LangHex):$($Script:Clsid)$($Script:ProfileGuid)"
$Script:TipStringHex = "0x$($Script:TipString)"
$Script:InstallRoot = Join-Path $env:ProgramFiles 'PersianPhoneticImeSpike'
$Script:AppKey   = 'HKCU:\Software\PersianPhoneticIme'
$Script:SpikeKey = 'HKCU:\Software\PersianPhoneticIme\Spike'

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
    if (-not ('PimeSpike.InputDll' -as [type])) {
        Add-Type -Namespace PimeSpike -Name InputDll -MemberDefinition @'
[DllImport("input.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool InstallLayoutOrTip(string psz, uint dwFlags);
'@
    }
    return [PimeSpike.InputDll]
}

# ILOT_UNINSTALL = 0x1 (documented as "same as ILOT_DISABLED")
function Set-TipEnabledForUser([bool]$Enabled) {
    $flags = if ($Enabled) { 0 } else { 1 }
    # The docs show both "0429:{..}{..}" and "0x0407:..." spellings; record which one works.
    foreach ($s in @($Script:TipString, $Script:TipStringHex)) {
        if ((Get-InputDll)::InstallLayoutOrTip($s, [uint32]$flags)) {
            Write-Host "InstallLayoutOrTip('$s', $flags) = TRUE"
            return $true
        }
        Write-Warning "InstallLayoutOrTip('$s', $flags) returned FALSE"
    }
    return $false
}

# All spike behaviour switches (HKCU\Software\PersianPhoneticIme\Spike), read by the DLL at each
# activation. Missing keys in $Values are reset to off, so every call states a complete variant.
$Script:SpikeFlags = 'EnterPasses', 'EatPrintable', 'BidiKeep', 'SpaceInComposition', 'PlainInsert', 'NoAttribute', 'NoComposition'
function Set-SpikeOptions([hashtable]$Values) {
    New-Item -Path $Script:SpikeKey -Force | Out-Null
    foreach ($f in $Script:SpikeFlags) {
        Set-ItemProperty $Script:SpikeKey -Name $f -Type DWord -Value ([int][bool]$Values[$f])
    }
    $mark = if ($Values['BidiMark']) { [string]$Values['BidiMark'] } else { 'none' }
    Set-ItemProperty $Script:SpikeKey -Name BidiMark -Type String -Value $mark
}

function Show-SpikeOptions {
    $v = Get-ItemProperty $Script:SpikeKey -ErrorAction SilentlyContinue
    if (-not $v) { Write-Host '(no spike options set)'; return }
    $on = $Script:SpikeFlags | Where-Object { $v.$_ -eq 1 }
    Write-Host ("BidiMark={0}  on: {1}" -f $v.BidiMark, $(if ($on) { $on -join ', ' } else { '(none)' }))
}
