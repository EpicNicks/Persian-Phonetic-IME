# Build both product TIP DLLs and stage crates\pime-tip\dist\{x64,x86,scripts,pime.wsb}.
# Safe on the dev host: builds and tests only, registers nothing, loads no DLL.
param([switch]$SkipTests)
# Not 'Stop': under Windows PowerShell 5.1 cargo's progress output on stderr would become
# terminating errors. Native failures are caught via $LASTEXITCODE.
$ErrorActionPreference = 'Continue'
$crate = Split-Path $PSScriptRoot -Parent
$root = Split-Path (Split-Path $crate -Parent) -Parent
$env:CARGO_TARGET_DIR = Join-Path $root 'target-tipA'
$dist = Join-Path $crate 'dist'

Push-Location $root
try {
    if (-not $SkipTests) {
        cargo test -p pime-tip; if ($LASTEXITCODE) { throw 'cargo test failed' }
        cargo clippy -p pime-tip --all-targets -- -D warnings; if ($LASTEXITCODE) { throw 'clippy failed' }
    }
    foreach ($t in @(@{ triple = 'x86_64-pc-windows-msvc'; dir = 'x64' },
                     @{ triple = 'i686-pc-windows-msvc';   dir = 'x86' })) {
        cargo build -p pime-tip --release --target $t.triple; if ($LASTEXITCODE) { throw "build $($t.triple) failed" }
        $out = Join-Path $dist $t.dir
        New-Item -ItemType Directory -Force $out | Out-Null
        $src = Join-Path $env:CARGO_TARGET_DIR "$($t.triple)\release"
        Copy-Item (Join-Path $src 'pime_tip.dll') $out -Force -ErrorAction Stop
        Copy-Item (Join-Path $src 'pime_tip.pdb') $out -Force -ErrorAction SilentlyContinue
    }
    $distScripts = Join-Path $dist 'scripts'
    New-Item -ItemType Directory -Force $distScripts | Out-Null
    Get-ChildItem $PSScriptRoot -File | Where-Object Name -ne 'pime.wsb' |
        Copy-Item -Destination $distScripts -Force -ErrorAction Stop
    # The sandbox config maps dist\ itself, so its HostFolder must be this checkout's path.
    $wsb = (Get-Content (Join-Path $PSScriptRoot 'pime.wsb') -Raw).Replace('@DIST@', $dist)
    [IO.File]::WriteAllText((Join-Path $dist 'pime.wsb'), $wsb, (New-Object Text.UTF8Encoding $false))
    Write-Host "Staged in $dist. Double-click $(Join-Path $dist 'pime.wsb') to open Windows Sandbox."
} finally { Pop-Location }
