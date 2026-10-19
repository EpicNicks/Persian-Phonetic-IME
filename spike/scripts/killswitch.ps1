# Toggle the kill switch HKCU\Software\PersianPhoneticIme\Disabled (checked in ActivateEx).
# Safe anywhere: it only writes the app's own HKCU key. Takes effect at the next activation
# (switch keyboards away and back, or restart the app).
param([switch]$On, [switch]$Off, [switch]$Machine)
. (Join-Path $PSScriptRoot 'common.ps1')
$key = if ($Machine) { 'HKLM:\Software\PersianPhoneticIme' } else { $Script:AppKey }
if ($On -eq $Off) { throw 'Pass exactly one of -On / -Off' }
New-Item -Path $key -Force | Out-Null
Set-ItemProperty $key -Name Disabled -Type DWord -Value ([int][bool]$On)
Write-Host "$key\Disabled = $([int][bool]$On)"
