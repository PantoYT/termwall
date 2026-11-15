# Builds build\termwall-setup-<version>.exe: termwall.exe and termwallw.exe (cargo, release) with
# the page, packed by Inno Setup. Runs locally (Rust and Inno Setup installed) and in GitHub Actions.
#
#     powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
# The version comes from Cargo.toml. ISCC.exe is looked up on PATH and in Inno Setup's usual
# folders; $env:ISCC overrides. The MSVC toolchain links WebView2's loader into the exe; with the
# GNU toolchain WebView2Loader.dll is staged next to it.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$build = Join-Path $root 'build'
$stage = Join-Path $build 'stage'

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "termwall $version"

# 1. the program
Push-Location $root
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$release = Join-Path $target 'release'

# 2. the stage: what gets installed
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($f in 'termwall.exe', 'termwallw.exe') { Copy-Item (Join-Path $release $f) $stage }
if (rustc -vV | Select-String 'host: .*-gnu') {
    $loader = Get-ChildItem $release -Filter 'WebView2Loader.dll' -Recurse | Select-Object -First 1
    if (-not $loader) { throw 'WebView2Loader.dll not found for the GNU build' }
    Copy-Item $loader.FullName $stage
}
foreach ($f in 'index.html', 'logos.js', 'project.json', 'README.md', 'LICENSE') {
    Copy-Item (Join-Path $root $f) $stage
}
Copy-Item (Join-Path $root 'fonts') $stage -Recurse  # bundled web fonts and their licenses

# 3. the staged program must really run
& (Join-Path $stage 'termwall.exe') --selftest
if ($LASTEXITCODE -ne 0) { throw 'selftest failed in the staged program' }
Remove-Item (Join-Path $stage 'selftest-tmp') -Recurse -Force -ErrorAction SilentlyContinue

# 4. Inno Setup
$iscc = $env:ISCC
if (-not $iscc) { $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
foreach ($c in "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
               "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") {
    if (-not $iscc -and (Test-Path $c)) { $iscc = $c }
}
if (-not $iscc) { throw 'ISCC.exe (Inno Setup 6) not found - winget install JRSoftware.InnoSetup' }
& $iscc "/DAppVersion=$version" (Join-Path $PSScriptRoot 'termwall.iss')
if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }

$exe = Join-Path $build "termwall-setup-$version.exe"
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash
Write-Host "built $exe"
Write-Host "sha256 $hash"
