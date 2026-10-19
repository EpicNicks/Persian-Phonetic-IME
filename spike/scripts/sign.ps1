# VM ONLY. Self-sign the installed spike DLLs and trust the certificate machine-wide, to test
# whether immersive hosts (Settings / Start search) load a signed-but-not-publicly-trusted TIP.
# Re-run register.ps1 first (it copies fresh unsigned DLLs), then this, then restart the hosts.
param([switch]$Force)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-SafeMachine -Force:$Force
Assert-Admin

$cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -eq 'CN=PimeSpike Test Signing' | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=PimeSpike Test Signing' `
        -CertStoreLocation Cert:\LocalMachine\My -NotAfter (Get-Date).AddMonths(6)
    $tmp = Join-Path $env:TEMP 'pimespike.cer'
    Export-Certificate -Cert $cert -FilePath $tmp | Out-Null
    Import-Certificate -FilePath $tmp -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    Import-Certificate -FilePath $tmp -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
}
Get-ChildItem $Script:InstallRoot -Recurse -Filter pime_spike.dll | ForEach-Object {
    $r = Set-AuthenticodeSignature -FilePath $_.FullName -Certificate $cert -HashAlgorithm SHA256
    Write-Host "$($_.FullName): $($r.Status)"
}
Write-Host 'Signed. Restart Settings / sign out and in, then retry the immersive hosts.'
