# Build and assemble the Windows app in build\windows\Sayso, and optionally the installer.
#
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1              release build
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Debug       debug app (faster), release engine
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Installer   also dist\Sayso-<version>-windows-x64-setup.exe
#
# The folder holds Sayso.exe, the speech engine SaysoEngine.exe with the
# runtime files it needs (DirectML.dll and the Visual C++ runtime), and the license. It runs from where it is, so it is
# also the portable build. The installer needs Inno Setup 6 (ISCC.exe on PATH,
# or in its default folder). Set SAYSO_BUNDLE_DIR to use another folder.
#
# Signing: set SAYSO_SIGN_CERT to a .pfx file and SAYSO_SIGN_PASSWORD to its
# password, and the exes and the installer are signed with signtool. Without
# them nothing is signed, and Windows SmartScreen warns about the download.
param(
    [switch]$Debug,
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$profile = if ($Debug) { 'debug' } else { 'release' }

function Invoke-Checked([string]$what, [scriptblock]$block) {
    Write-Host "==> $what"
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

# CMake, Ninja, and libclang for the speech engine.
. (Join-Path $PSScriptRoot 'windows-env.ps1')
Invoke-Checked 'engine (cargo, release)' { cargo build --release --manifest-path native\portable\Cargo.toml }
if ($Debug) {
    Invoke-Checked 'app (cargo, debug)' { cargo build -p sayso-app }
} else {
    Invoke-Checked 'app (cargo, release)' { cargo build -p sayso-app --release }
}

$bundle = if ($env:SAYSO_BUNDLE_DIR) { $env:SAYSO_BUNDLE_DIR } else { Join-Path $root 'build\windows' }
$app = Join-Path $bundle 'Sayso'
if (Test-Path $app) { Remove-Item -Recurse -Force $app }
New-Item -ItemType Directory -Force $app | Out-Null

Copy-Item "target\$profile\sayso.exe" (Join-Path $app 'Sayso.exe')
$engineDir = 'native\portable\target\release'
Copy-Item (Join-Path $engineDir 'SaysoEngine.exe') $app
# DirectML.dll, which the ONNX Runtime build of the engine loads at start, sits beside it.
Get-ChildItem $engineDir -Filter *.dll | ForEach-Object { Copy-Item $_.FullName $app }
# Both exes need the Visual C++ runtime. Ship it beside them (app-local), so
# the app also runs where vc_redist is not installed.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "the Visual C++ runtime was not found in $vs\VC\Redist" }
foreach ($dll in 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll') {
    Copy-Item (Join-Path $crt.FullName $dll) $app
}
Copy-Item LICENSE (Join-Path $app 'LICENSE.txt')
Copy-Item assets\fonts\licenses -Recurse (Join-Path $app 'licenses')

function Sign-File([string]$path) {
    if (-not $env:SAYSO_SIGN_CERT) { return }
    $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if (-not $signtool) {
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1
    }
    if (-not $signtool) { throw 'SAYSO_SIGN_CERT is set, but signtool.exe was not found' }
    & $signtool sign /f $env:SAYSO_SIGN_CERT /p $env:SAYSO_SIGN_PASSWORD /fd sha256 /tr http://timestamp.digicert.com /td sha256 $path
    if ($LASTEXITCODE -ne 0) { throw "signing $path failed" }
}
Sign-File (Join-Path $app 'Sayso.exe')
Sign-File (Join-Path $app 'SaysoEngine.exe')
Write-Host "==> app folder: $app"

if ($Installer) {
    $iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if (-not $iscc) {
        $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) was not found. Install it, or leave out -Installer.' }
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    Invoke-Checked 'installer (Inno Setup)' {
        & $iscc /Qp "/DAppVersion=$version" "/DSourceDir=$app" "/DOutputDir=$dist" packaging\windows\Sayso.iss
    }
    $setup = Join-Path $dist "Sayso-$version-windows-x64-setup.exe"
    Sign-File $setup
    $hash = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLower()
    Set-Content -Encoding ascii -NoNewline -Path "$setup.sha256" -Value "$hash  $(Split-Path -Leaf $setup)`n"
    Write-Host "==> installer: $setup"
}
