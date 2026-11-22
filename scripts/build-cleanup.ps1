# Build the Rust cleanup runtime (native/cleanup) for its release file and the
# npm package @justsayso/cleanup: the Windows counterpart of
# scripts/build-cleanup.sh. The app does not use it: Sayso runs the Go runtime
# cmd/sayso-cleanup (scripts/bundle-windows.ps1).
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-cleanup.ps1 [-Out build\cleanup]
#
# The folder gets sayso-cleanup.exe and its AVX and baseline builds
# (sayso-cleanup-avx.exe, sayso-cleanup-baseline.exe; llama.cpp compiles its
# kernels for one instruction set, issue #37), and the Visual C++ runtime that
# they need. Then scripts\package-cleanup.ps1 -Folder <folder> makes the zip.
#
# Signing: set SAYSO_SIGN_CERT to a .pfx file and SAYSO_SIGN_PASSWORD to its
# password, and the exes are signed with signtool.
param(
    [string]$Out
)
$ErrorActionPreference = 'Stop'
if ($Out) { $Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) }
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not $Out) { $Out = Join-Path $root 'build\cleanup' }

function Invoke-Checked([string]$what, [scriptblock]$block) {
    Write-Host "==> $what"
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

# CMake, Ninja, and libclang for llama.cpp.
. (Join-Path $PSScriptRoot 'windows-env.ps1')
$avxOff = @('GGML_AVX2', 'GGML_FMA', 'GGML_F16C', 'GGML_BMI2')
$baselineOff = @('GGML_SSE42', 'GGML_AVX') + $avxOff
# llama.cpp is slow in a debug build, so the runtime is always a release build.
$builds = @(
    @{ Name = 'sayso-cleanup.exe'; Dir = 'native\cleanup\target'; Off = @() },
    @{ Name = 'sayso-cleanup-avx.exe'; Dir = 'native\cleanup\target\avx'; Off = $avxOff },
    @{ Name = 'sayso-cleanup-baseline.exe'; Dir = 'native\cleanup\target\baseline'; Off = $baselineOff }
)
foreach ($build in $builds) {
    $env:GGML_NATIVE = 'OFF'
    foreach ($var in $build.Off) { Set-Item "env:$var" 'OFF' }
    try {
        Invoke-Checked "cleanup runtime $($build.Name) (cargo, release)" {
            cargo build --release --manifest-path native\cleanup\Cargo.toml --target-dir $build.Dir
        }
    } finally {
        foreach ($var in @('GGML_NATIVE') + $build.Off) { Remove-Item "env:$var" -ErrorAction SilentlyContinue }
    }
}

if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
foreach ($build in $builds) {
    Copy-Item (Join-Path $build.Dir 'release\sayso-cleanup.exe') (Join-Path $Out $build.Name)
}
# The exes need the Visual C++ runtime. It ships beside them (app-local).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "the Visual C++ runtime was not found in $vs\VC\Redist" }
foreach ($dll in 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll') {
    Copy-Item (Join-Path $crt.FullName $dll) $Out
}

if ($env:SAYSO_SIGN_CERT) {
    $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if (-not $signtool) {
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1
    }
    if (-not $signtool) { throw 'SAYSO_SIGN_CERT is set, but signtool.exe was not found' }
    foreach ($build in $builds) {
        & $signtool sign /f $env:SAYSO_SIGN_CERT /p $env:SAYSO_SIGN_PASSWORD /fd sha256 /tr http://timestamp.digicert.com /td sha256 (Join-Path $Out $build.Name)
        if ($LASTEXITCODE -ne 0) { throw "signing $($build.Name) failed" }
    }
}
Write-Host "==> cleanup runtime builds: $Out"
