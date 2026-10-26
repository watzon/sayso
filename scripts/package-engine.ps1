# Make the engine release file for Windows: a zip of the speech engine with the
# DLLs that it needs, its README, and its licenses, and the checksum.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package-engine.ps1 [-Folder <app folder>] [-Out <folder>]
#
#   dist\sayso-engine-<version>-windows-x64.zip
#   dist\sayso-engine-<version>-windows-x64.zip.sha256
#
# The zip holds the folder sayso-engine-<version>\ with sayso-engine.exe, its
# DLLs (sherpa-onnx, ONNX Runtime, and the Visual C++ runtime), README.md,
# LICENSE, and licenses\. The script copies these files from the app folder
# that scripts\bundle-windows.ps1 makes (build\windows\Sayso, or Sayso in
# SAYSO_BUNDLE_DIR), so the exe is the one that was signed there. It never
# builds an engine. The zip is better than a tarball here: Windows opens a zip
# without another program. The macOS and Linux files are made by
# scripts/package-engine.sh.
param(
    [string]$Folder,
    [string]$Out
)
$ErrorActionPreference = 'Stop'
# Make the paths of the arguments absolute before the script changes folder.
if ($Folder) { $Folder = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Folder) }
if ($Out) { $Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) }
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Folder) {
    $bundle = if ($env:SAYSO_BUNDLE_DIR) { $env:SAYSO_BUNDLE_DIR } else { Join-Path $root 'build\windows' }
    $Folder = Join-Path $bundle 'Sayso'
}
if (-not $Out) { $Out = Join-Path $root 'dist' }

$exe = Join-Path $Folder 'sayso-engine.exe'
if (-not (Test-Path $exe)) { throw "the engine $exe does not exist. Run scripts\bundle-windows.ps1 first." }

$name = "sayso-engine-$version"
$file = "$name-windows-x64.zip"
$stage = Join-Path ([IO.Path]::GetTempPath()) "sayso-engine-stage-$([guid]::NewGuid())"
$dir = Join-Path $stage $name
try {
    New-Item -ItemType Directory -Force (Join-Path $dir 'licenses') | Out-Null
    New-Item -ItemType Directory -Force $Out | Out-Null

    Copy-Item $exe $dir
    # Every DLL of the app folder: the sherpa-onnx and ONNX Runtime DLLs, and the
    # Visual C++ runtime that bundle-windows.ps1 ships app-local.
    Get-ChildItem $Folder -Filter '*.dll' | Copy-Item -Destination $dir
    foreach ($dll in 'sherpa-onnx-c-api.dll', 'vcruntime140.dll') {
        if (-not (Test-Path (Join-Path $dir $dll))) { throw "$dll is not in $Folder. The engine does not start without it." }
    }
    Copy-Item LICENSE (Join-Path $dir 'LICENSE')
    # The license that the app folder ships for the engine (whisper.cpp is ggml's code).
    $ggml = Join-Path $Folder 'licenses\ggml.txt'
    if (-not (Test-Path $ggml)) { throw "$ggml does not exist" }
    Copy-Item $ggml (Join-Path $dir 'licenses')

    # The README: the same text for each system, with the command for this one.
    $run = @'
$env:SAYSO_MODELS_DIR = "$env:LOCALAPPDATA\sayso-models"
.\sayso-engine.exe
'@
    $hello = @'
$env:SAYSO_MODELS_DIR = "$env:LOCALAPPDATA\sayso-models"
& { '{"v":1,"id":"1","type":"hello"}'; Start-Sleep 2 } | .\sayso-engine.exe
'@
    $requirements = '64-bit Windows. Keep the DLLs in this folder next to `sayso-engine.exe`. They include the Visual C++ runtime, so the computer needs no other install.'
    $readme = (Get-Content -Raw 'packaging\engine\README.md.in').TrimEnd()
    $readme = $readme.Replace('@VERSION@', $version).Replace('@SYSTEM@', 'Windows').Replace('@REQUIREMENTS@', $requirements).Replace('@RUN@', $run.TrimEnd()).Replace('@HELLO@', $hello.TrimEnd())
    [IO.File]::WriteAllText((Join-Path $dir 'README.md'), $readme + "`n", (New-Object Text.UTF8Encoding $false))

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = Join-Path $Out $file
    if (Test-Path $zip) { Remove-Item -Force $zip }
    # The last argument keeps the folder sayso-engine-<version> at the top of the zip.
    [IO.Compression.ZipFile]::CreateFromDirectory($dir, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)

    $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    Set-Content -Encoding ascii -NoNewline -Path "$zip.sha256" -Value "$hash  $file`n"
    Write-Host "==> engine file: $zip"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $stage
}
