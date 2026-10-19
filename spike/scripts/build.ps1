# Build both spike DLLs and stage them in spike\dist\{x64,x86}. Safe on the dev host:
# builds only, registers nothing.
param([switch]$SkipTests)
# Not 'Stop': under Windows PowerShell 5.1 cargo's progress output on stderr would become
# terminating errors when redirected. Native failures are caught via $LASTEXITCODE.
$ErrorActionPreference = 'Continue'
$spike = Split-Path $PSScriptRoot -Parent
$root = Split-Path $spike -Parent
$env:CARGO_TARGET_DIR = Join-Path $root 'target-ws1'

Push-Location $spike
try {
    if (-not $SkipTests) {
        cargo test; if ($LASTEXITCODE) { throw 'cargo test failed' }
        cargo clippy --all-targets -- -D warnings; if ($LASTEXITCODE) { throw 'clippy failed' }
    }
    foreach ($t in @(@{ triple = 'x86_64-pc-windows-msvc'; dir = 'x64' },
                     @{ triple = 'i686-pc-windows-msvc';   dir = 'x86' })) {
        cargo build --release --target $t.triple; if ($LASTEXITCODE) { throw "build $($t.triple) failed" }
        $out = Join-Path $spike "dist\$($t.dir)"
        New-Item -ItemType Directory -Force $out | Out-Null
        $src = Join-Path $env:CARGO_TARGET_DIR "$($t.triple)\release"
        Copy-Item (Join-Path $src 'pime_spike.dll') $out -Force -ErrorAction Stop
        Copy-Item (Join-Path $src 'pime_spike.pdb') $out -Force -ErrorAction SilentlyContinue
    }
    $distScripts = Join-Path $spike 'dist\scripts'
    New-Item -ItemType Directory -Force $distScripts | Out-Null
    Copy-Item (Join-Path $spike 'scripts\*') $distScripts -Force -ErrorAction Stop
    Write-Host "Staged in $(Join-Path $spike 'dist'). Copy that folder into the VM (or map it in Sandbox)."
} finally { Pop-Location }
