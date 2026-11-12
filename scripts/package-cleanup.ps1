# Make the cleanup runtime release file for Windows: a zip of the runtime
# sayso-cleanup with the Visual C++ runtime that it needs, its README, and its
# licenses, and the checksum.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package-cleanup.ps1 [-Folder <app folder>] [-Out <folder>] [-NoSmoke]
#
#   dist\sayso-cleanup-<version>-windows-x64.zip
#   dist\sayso-cleanup-<version>-windows-x64.zip.sha256
#
# The zip holds the folder sayso-cleanup-<version>\ with sayso-cleanup.exe and
# its AVX and baseline builds (sayso-cleanup-avx.exe, sayso-cleanup-baseline.exe;
# llama.cpp is compiled for one instruction set, issue #37, and the SDK starts
# the best build that the processor runs), the Visual C++ runtime DLLs,
# README.md, LICENSE, and licenses\. The script copies these files from the app
# folder that scripts\bundle-windows.ps1 makes (build\windows\Sayso, or Sayso
# in SAYSO_BUNDLE_DIR), so the exes are the ones that were signed there. It
# never builds a runtime. By default the script starts each build in the zip
# and sends it a hello request; -NoSmoke skips that. The zip is better than a
# tarball here: Windows opens a zip without another program. The macOS and
# Linux files are made by scripts/package-cleanup.sh.
param(
    [string]$Folder,
    [string]$Out,
    [switch]$NoSmoke
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

$names = 'sayso-cleanup.exe', 'sayso-cleanup-avx.exe', 'sayso-cleanup-baseline.exe'
$exes = $names | ForEach-Object { Join-Path $Folder $_ }
foreach ($exe in $exes) {
    if (-not (Test-Path $exe)) { throw "the runtime $exe does not exist. Run scripts\bundle-windows.ps1 first." }
}
# The Visual C++ runtime that bundle-windows.ps1 ships app-local.
$crt = 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_1.dll'
foreach ($dll in $crt) {
    if (-not (Test-Path (Join-Path $Folder $dll))) { throw "$dll is not in $Folder. The runtime does not start without it." }
}

$name = "sayso-cleanup-$version"
$file = "$name-windows-x64.zip"
$stage = Join-Path ([IO.Path]::GetTempPath()) "sayso-cleanup-stage-$([guid]::NewGuid())"
$dir = Join-Path $stage $name
try {
    New-Item -ItemType Directory -Force (Join-Path $dir 'licenses') | Out-Null
    New-Item -ItemType Directory -Force $Out | Out-Null

    Copy-Item $exes $dir
    foreach ($dll in $crt) { Copy-Item (Join-Path $Folder $dll) $dir }
    Copy-Item LICENSE (Join-Path $dir 'LICENSE')
    # llama.cpp is linked statically, and it has the same MIT license text as ggml.
    Copy-Item 'crates\sayso-engine\licenses\ggml.txt' (Join-Path $dir 'licenses')

    # Start each build and send it a hello request. The runtime needs no model to
    # answer. A processor without the instructions of a build stops it with
    # STATUS_ILLEGAL_INSTRUCTION; the SDK picks the best build that runs.
    if (-not $NoSmoke) {
        $env:SAYSO_CLEANUP_LOG = 'off'
        foreach ($exeName in $names) {
            $info = New-Object Diagnostics.ProcessStartInfo (Join-Path $dir $exeName)
            $info.RedirectStandardInput = $true
            $info.RedirectStandardOutput = $true
            $info.RedirectStandardError = $true
            $info.UseShellExecute = $false
            $process = [Diagnostics.Process]::Start($info)
            $process.StandardInput.WriteLine('{"v":1,"id":"1","type":"hello"}')
            $process.StandardInput.Close()
            $reply = $process.StandardOutput.ReadLine()
            if (-not $process.WaitForExit(20000)) { $process.Kill(); throw "$exeName did not stop when stdin closed" }
            if ($process.ExitCode -eq -1073741795) {
                Write-Host "==> ${exeName}: this processor does not run it (illegal instruction), start test skipped"
                continue
            }
            Write-Host "==> ${exeName}: $reply"
            if (-not $reply -or $reply -notmatch '"type":"hello"') { throw "$exeName gave no hello reply (exit code $($process.ExitCode))" }
        }
    } else {
        Write-Host '==> start test skipped'
    }

    # The README: the same text for each system, with the command for this one.
    $hello = @'
& { '{"v":1,"id":"1","type":"hello"}'; Start-Sleep 1 } | .\sayso-cleanup.exe
'@
    $requirements = '64-bit Windows. Keep the DLLs in this folder next to `sayso-cleanup.exe`. They are the Visual C++ runtime, so the computer needs no other install. The runtime uses the CPU only.'
    $builds = 'This file has three builds, for three levels of the processor: `sayso-cleanup.exe` (AVX2, FMA, F16C, and BMI2), `sayso-cleanup-avx.exe` (AVX and SSE 4.2), and `sayso-cleanup-baseline.exe` (x86-64 only). A processor that lacks the instructions of a build stops it with an illegal instruction. The SDK starts the best build that the processor runs.'
    $readme = (Get-Content -Raw 'packaging\cleanup\README.md.in').TrimEnd()
    $readme = $readme.Replace('@VERSION@', $version).Replace('@SYSTEM@', 'Windows').Replace('@REQUIREMENTS@', $requirements).Replace('@BUILDS@', $builds).Replace('@HELLO@', $hello.TrimEnd())
    [IO.File]::WriteAllText((Join-Path $dir 'README.md'), $readme + "`n", (New-Object Text.UTF8Encoding $false))

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = Join-Path $Out $file
    if (Test-Path $zip) { Remove-Item -Force $zip }
    # The last argument keeps the folder sayso-cleanup-<version> at the top of the zip.
    [IO.Compression.ZipFile]::CreateFromDirectory($dir, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)

    $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    Set-Content -Encoding ascii -NoNewline -Path "$zip.sha256" -Value "$hash  $file`n"
    Write-Host "==> cleanup runtime file: $zip"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $stage
}
