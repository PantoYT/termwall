# Builds build\termwall-setup-<version>.exe: termwall's files + an embeddable Python with psutil,
# packed by Inno Setup. Runs locally (with Inno Setup installed) and in GitHub Actions.
#
#     powershell -ExecutionPolicy Bypass -File installer\build.ps1
#
# The version comes from termwall_api.py (__version__). ISCC.exe is looked up on PATH and in
# Inno Setup's usual folders; $env:ISCC overrides.

param([string]$PythonVersion = '3.13.7')

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = Split-Path $PSScriptRoot -Parent
$build = Join-Path $root 'build'
$stage = Join-Path $build 'stage'
$py = Join-Path $stage 'python'

$version = (Select-String -Path (Join-Path $root 'termwall_api.py') -Pattern '^__version__ = "(.+)"').Matches[0].Groups[1].Value
Write-Host "termwall $version, Python $PythonVersion"

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $py | Out-Null

# 1. embeddable Python (python.org, signed by the PSF)
$zip = Join-Path $build "python-$PythonVersion-embed-amd64.zip"
if (-not (Test-Path $zip)) {
    Invoke-WebRequest "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip" -OutFile $zip -UseBasicParsing
}
Expand-Archive $zip -DestinationPath $py
# the embeddable build ignores site-packages until its ._pth file says otherwise
$pth = Get-ChildItem $py -Filter 'python*._pth' | Select-Object -First 1
$tag = $pth.BaseName                                             # e.g. python313
Set-Content -Path $pth.FullName -Encoding ascii -Value @("$tag.zip", '.', 'Lib\site-packages', '..', 'import site')

# 2. psutil for that Python, as a ready-made wheel (no compiler needed)
$short = ($PythonVersion -split '\.')[0..1] -join '.'
python -m pip install psutil --target (Join-Path $py 'Lib\site-packages') --only-binary=:all: `
    --platform win_amd64 --python-version $short --implementation cp --quiet --disable-pip-version-check
if ($LASTEXITCODE -ne 0) { throw 'pip could not fetch psutil' }

# 3. termwall itself
foreach ($f in 'index.html', 'logos.js', 'termwall_api.py', 'project.json', 'README.md', 'LICENSE', 'termwall-api.vbs') {
    Copy-Item (Join-Path $root $f) $stage
}

# 4. the bundled Python must really run termwall
& (Join-Path $py 'python.exe') (Join-Path $stage 'termwall_api.py') --selftest
if ($LASTEXITCODE -ne 0) { throw 'selftest failed with the bundled Python' }

# 5. Inno Setup
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
