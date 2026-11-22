# Build and assemble the Windows app in build\windows\Sayso, and optionally the installer.
#
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1              release build
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Debug       debug app (faster), release engine
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Installer   also dist\Sayso-<version>-windows-x64-setup.exe
#
# The folder holds Sayso.exe (Go, CGO_ENABLED=0), the speech engine
# sayso-engine.exe (Go), the native libraries that the engine loads at run time
# (sherpa-onnx, ONNX Runtime, whisper.cpp, ggml: DLLs), the Visual C++ runtime,
# the licenses, and llama\: the cleanup runtime sayso-cleanup.exe (Go) with the
# pinned llama.cpp DLLs (internal/llamalib), libffi, and a copy of the Visual
# C++ runtime. The ggml of llama.cpp stays in llama\, apart from the ggml of
# the engine. Nothing here runs Cargo. It runs
# from where it is, so it is also the portable build. The installer needs Inno
# Setup 6 (ISCC.exe on PATH, or in its default folder). Set SAYSO_BUNDLE_DIR to
# use another folder.
#
# SAYSO_NATIVE_LIBS_DIR is the folder with the engine's DLLs (every *.dll in it
# is copied beside the engine). It is required for a release build.
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

function Invoke-Checked([string]$what, [scriptblock]$block) {
    Write-Host "==> $what"
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

$env:CGO_ENABLED = '0'

$buildDir = Join-Path $root 'build'
New-Item -ItemType Directory -Force $buildDir | Out-Null
if ($Debug) {
    Invoke-Checked 'app (go, debug)' { go build -o build\sayso.exe .\cmd\sayso }
} else {
    # -H=windowsgui makes a GUI program with no console window.
    Invoke-Checked 'app (go, release)' { go build -tags mygo_noinspector -ldflags='-s -w -H=windowsgui' -o build\sayso.exe .\cmd\sayso }
}
Invoke-Checked 'engine (go, release)' { go build -ldflags='-s -w' -o build\sayso-engine.exe .\cmd\sayso-engine }
# The cleanup runtime loads llama.cpp with yzma through libffi. ffi_no_embed:
# it loads the libffi-8.dll beside it, not a copy written to the user cache.
Invoke-Checked 'cleanup runtime (go, release)' { go build -tags ffi_no_embed -ldflags='-s -w' -o build\sayso-cleanup.exe .\cmd\sayso-cleanup }
Invoke-Checked 'llama-libs tool (go)' { go build -o build\llama-libs-tool.exe .\scripts\llama-libs }
$llamaLibs = & build\llama-libs-tool.exe fetch -os windows -arch amd64 -out build\llama-libs
if ($LASTEXITCODE -ne 0) { throw 'the llama.cpp libraries could not be fetched' }

$bundle = if ($env:SAYSO_BUNDLE_DIR) { $env:SAYSO_BUNDLE_DIR } else { Join-Path $root 'build\windows' }
$app = Join-Path $bundle 'Sayso'
if (Test-Path $app) { Remove-Item -Recurse -Force $app }
New-Item -ItemType Directory -Force $app | Out-Null

Copy-Item 'build\sayso.exe' (Join-Path $app 'Sayso.exe')
# The app finds the engine next to its exe.
Copy-Item 'build\sayso-engine.exe' (Join-Path $app 'sayso-engine.exe')
# The engine loads its native libraries from its own folder.
if ($env:SAYSO_NATIVE_LIBS_DIR) {
    Get-ChildItem $env:SAYSO_NATIVE_LIBS_DIR -Filter '*.dll' | Copy-Item -Destination $app
} elseif (-not $Debug) {
    throw 'set SAYSO_NATIVE_LIBS_DIR to the folder with the engine DLLs (sherpa-onnx, onnxruntime, whisper, ggml)'
}
# The Visual C++ runtime that the engine's DLLs and the llama.cpp DLLs need,
# app-local, so the app also runs where vc_redist is not installed.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "the Visual C++ runtime was not found in $vs\VC\Redist" }
$crtDlls = 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll'
foreach ($dll in $crtDlls) {
    Copy-Item (Join-Path $crt.FullName $dll) $app
}
# The app finds the cleanup runtime in llama\. Windows looks for the DLLs of a
# DLL in its own folder first (LOAD_WITH_ALTERED_SEARCH_PATH) and for
# libffi-8.dll in the folder of the exe, so llama\ has every DLL that they need,
# with its own copy of the Visual C++ runtime, and none of the app folder.
$llama = Join-Path $app 'llama'
New-Item -ItemType Directory -Force $llama | Out-Null
Copy-Item 'build\sayso-cleanup.exe' (Join-Path $llama 'sayso-cleanup.exe')
Get-ChildItem $llamaLibs -Filter '*.dll' | Copy-Item -Destination $llama
& build\llama-libs-tool.exe ffi -os windows -arch amd64 $llama
if ($LASTEXITCODE -ne 0) { throw 'libffi could not be copied' }
foreach ($dll in $crtDlls) {
    Copy-Item (Join-Path $crt.FullName $dll) $llama
}
Copy-Item LICENSE (Join-Path $app 'LICENSE.txt')
New-Item -ItemType Directory -Force (Join-Path $app 'licenses') | Out-Null
# whisper.cpp and llama.cpp are ggml's code, under ggml's MIT License.
# sherpa-onnx (Apache-2.0) and ONNX Runtime (MIT) come as the DLLs above. The
# cleanup runtime adds libffi (MIT) and the LLVM OpenMP runtime (libomp.dll,
# Apache-2.0 with LLVM exceptions).
foreach ($name in 'ggml.txt', 'sherpa-onnx.txt', 'onnxruntime.txt', 'libffi.txt') {
    Copy-Item (Join-Path packaging\licenses $name) (Join-Path $app 'licenses')
}
Copy-Item (Join-Path $llamaLibs 'LICENSE-LLVM-OpenMP') (Join-Path $app 'licenses\llvm-openmp.txt')

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
Sign-File (Join-Path $app 'sayso-engine.exe')
# The cleanup runtime and the DLLs of llama\, but not the Visual C++ runtime,
# which Microsoft signs (the runtime does not check the hashes of a package
# folder; internal/llamalib).
Get-ChildItem $llama -Include '*.exe', '*.dll' -Recurse | Where-Object { $crtDlls -notcontains $_.Name } | ForEach-Object { Sign-File $_.FullName }
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
