# Headless GNU verification. All helper files stay inside lag-audio/target.
param([switch]$UseLld)
$ErrorActionPreference = 'Stop'
$audioRoot = Split-Path -Parent $PSScriptRoot
$workspace = (Resolve-Path (Join-Path $audioRoot '../..')).Path
$reference = (Resolve-Path (Join-Path $workspace '../InstantReplay')).Path
$env:PATH = "$reference\ffmpeg;$workspace\vendor\mpv;$env:PATH"
if ($UseLld) {
    $sysroot = (& rustc --print sysroot).Trim()
    $bin = Join-Path $sysroot 'lib/rustlib/x86_64-pc-windows-gnu/bin'
    $env:LAG_RUST_LLD = Join-Path $bin 'rust-lld.exe'
    $linker = Join-Path $bin 'gcc-ld/ld.lld.exe'
    $helper = Join-Path $audioRoot 'target/dlltool.exe'
    New-Item -ItemType Directory -Force (Split-Path -Parent $helper) | Out-Null
    & rustc --edition=2021 (Join-Path $PSScriptRoot 'dlltool.rs') -o $helper -C "linker=$linker" -C linker-flavor=ld
    if ($LASTEXITCODE -ne 0) { throw 'dlltool adapter build failed' }
    # Forward slashes avoid quoting/escape problems in Cargo's whitespace-delimited RUSTFLAGS.
    $env:RUSTFLAGS = "-C dlltool=$($helper.Replace('\','/')) -C linker=$($linker.Replace('\','/')) -C linker-flavor=ld"
}
Push-Location $workspace
try {
    cargo fmt -p lag-audio -- --check
    if ($LASTEXITCODE -ne 0) { throw 'cargo fmt failed' }
    cargo test -p lag-audio --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
    cargo clippy -p lag-audio --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
} finally { Pop-Location }
