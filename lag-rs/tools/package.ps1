# Lag (Rust) release folder + installer.
#
#   powershell -ExecutionPolicy Bypass -File lag-rs\tools\package.ps1 -Version 2.0.0 [-Out <dir>] [-NoBuild] [-Installer]
#
# 1. builds the release exe (unless -NoBuild),
# 2. assembles the publish folder the installer packs (installer\Lag.iss, /DPublishDir=<Out>),
# 3. FAILS when any runtime file is missing - the app does not crash without them, it silently loses a
#    feature (no rnnoise/std.rnnn = noise suppression quietly off; no ffmpeg\ffmpeg.exe = no editor export,
#    thumbnails or carry-over re-encode). Settings audit 2026-11-17.
# 4. with -Installer runs ISCC (Inno Setup 6) to produce installer\Output\Lag-win-Setup.exe,
# 5. with -Portable zips the folder into installer\Output\Lag-win-Portable.zip (the release's second asset).
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Out = "",
    [switch]$NoBuild,
    [switch]$Installer,
    [switch]$Portable
)
$ErrorActionPreference = "Stop"
$lagRs = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $lagRs
$release = Join-Path $lagRs "target\release"
if (-not $Out) { $Out = Join-Path $repo "release-publish-rs" }

if (-not $NoBuild) {
    Push-Location $lagRs
    try {
        & cargo build --release -p lag-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}

if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null

Copy-Item (Join-Path $release "Lag.exe") $Out
Get-ChildItem $release -Filter *.dll | Copy-Item -Destination $Out
foreach ($sub in "ffmpeg", "rnnoise") {
    Copy-Item -Recurse (Join-Path $repo "InstantReplay\$sub") (Join-Path $Out $sub)
}
New-Item -ItemType Directory -Force (Join-Path $Out "Assets") | Out-Null
foreach ($f in "icon.ico", "save.wav", "screenshot.wav") {
    Copy-Item (Join-Path $repo "InstantReplay\Assets\$f") (Join-Path $Out "Assets")
}
Copy-Item (Join-Path $repo "LICENSE") $Out

$required = @(
    "Lag.exe", "libmpv-2.dll",
    "avcodec-61.dll", "avdevice-61.dll", "avfilter-10.dll", "avformat-61.dll", "avutil-59.dll",
    "postproc-58.dll", "swresample-5.dll", "swscale-8.dll",
    "ffmpeg\ffmpeg.exe", "ffmpeg\ffprobe.exe", "ffmpeg\LICENSE.txt",
    "rnnoise\std.rnnn", "rnnoise\LICENSE-rnnoise.txt",
    "Assets\save.wav", "Assets\screenshot.wav", "Assets\icon.ico", "LICENSE"
)
$missing = $required | Where-Object { -not (Test-Path (Join-Path $Out $_)) }
if ($missing) { throw "release folder is incomplete, missing: $($missing -join ', ')" }

# the exe must be the one just built, not a stale copy
$built = (Get-Item (Join-Path $release "Lag.exe")).LastWriteTimeUtc
$packed = (Get-Item (Join-Path $Out "Lag.exe")).LastWriteTimeUtc
if ($built -ne $packed) { throw "Lag.exe in the release folder is not the fresh build" }

# The release profile keeps line tables (readable panic backtraces / crash-dump symbolication from
# target\release\Lag.exe); the shipped copy drops them: ~105 MB -> ~29 MB. Code is unchanged, so the
# unstripped exe in target\release still symbolicates dumps of the shipped one.
$strip = @(
    (Get-Command strip.exe -ErrorAction SilentlyContinue | ForEach-Object Source),
    (Get-Command llvm-strip.exe -ErrorAction SilentlyContinue | ForEach-Object Source),
    (Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Recurse -Filter strip.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName)
) | Where-Object { $_ } | Select-Object -First 1
if ($strip) {
    & $strip --strip-debug (Join-Path $Out "Lag.exe")
    if ($LASTEXITCODE -ne 0) { throw "strip failed ($LASTEXITCODE)" }
    Write-Host "stripped debug info: $([math]::Round((Get-Item (Join-Path $Out 'Lag.exe')).Length / 1MB, 1)) MB"
} else {
    Write-Warning "strip not found: Lag.exe ships with its debug line tables (~105 MB)"
}
Write-Host "release folder OK: $Out ($($required.Count) required files present)"

if ($Installer) {
    $iscc = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue | ForEach-Object Source),
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found" }
    & $iscc (Join-Path $repo "installer\Lag.iss") "/DAppVersion=$Version" "/DPublishDir=$Out"
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
}

if ($Portable) {
    $outDir = Join-Path $repo "installer\Output"
    New-Item -ItemType Directory -Force $outDir | Out-Null
    $zip = Join-Path $outDir "Lag-win-Portable.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path (Join-Path $Out "*") -DestinationPath $zip -CompressionLevel Optimal
    Write-Host "portable zip: $zip"
}
