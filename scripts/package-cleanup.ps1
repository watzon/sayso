# Make the cleanup runtime release file for Windows: a zip of sayso-cleanup.exe
# with its llama.cpp DLLs, its licenses, and its checksum.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package-cleanup.ps1 [-Folder <app folder>] [-Out <folder>]
#
#   dist\sayso-cleanup-<version>-windows-x64.zip
#   dist\sayso-cleanup-<version>-windows-x64.zip.sha256
#
# The zip holds the folder sayso-cleanup-<version>\ with the files of the llama\
# folder of the app (scripts\bundle-windows.ps1): sayso-cleanup.exe, the
# llama.cpp DLLs, libffi-8.dll, and the Visual C++ runtime. The runtime finds
# its libraries beside its exe (internal/llamalib BundledDir), so the layout is
# the one of llama\. The files are the ones that were signed there. The script
# never builds the runtime. Before it packs, it runs hello on the copy with an
# empty cache folder, and stops when the runtime does not find its libraries in
# the file: a user must never wait for their download. The macOS and Linux
# files are made by scripts/package-cleanup.sh.
param(
    [string]$Folder,
    [string]$Out
)
$ErrorActionPreference = 'Stop'
if ($Folder) { $Folder = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Folder) }
if ($Out) { $Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out) }
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path internal\update\version.go -Pattern '^var Version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Folder) {
    $bundle = if ($env:SAYSO_BUNDLE_DIR) { $env:SAYSO_BUNDLE_DIR } else { Join-Path $root 'build\windows' }
    $Folder = Join-Path $bundle 'Sayso'
}
if (-not $Out) { $Out = Join-Path $root 'dist' }

$llama = Join-Path $Folder 'llama'
if (-not (Test-Path (Join-Path $llama 'sayso-cleanup.exe'))) { throw "$llama has no sayso-cleanup.exe. Run scripts\bundle-windows.ps1 first." }

$name = "sayso-cleanup-$version"
$file = "$name-windows-x64.zip"
$stage = Join-Path ([IO.Path]::GetTempPath()) "sayso-cleanup-stage-$([guid]::NewGuid())"
$dir = Join-Path $stage $name
$checkHome = Join-Path ([IO.Path]::GetTempPath()) "sayso-cleanup-check-$([guid]::NewGuid())"
try {
    New-Item -ItemType Directory -Force (Join-Path $dir 'licenses') | Out-Null
    New-Item -ItemType Directory -Force $Out | Out-Null

    Get-ChildItem $llama -File | Copy-Item -Destination $dir
    foreach ($dll in 'llama.dll', 'ggml.dll', 'ggml-base.dll', 'libffi-8.dll', 'vcruntime140.dll') {
        if (-not (Test-Path (Join-Path $dir $dll))) { throw "$dll is not in $llama. The runtime does not start without it." }
    }
    Copy-Item LICENSE (Join-Path $dir 'LICENSE')
    # llama.cpp has the MIT text of ggml, byte for byte.
    Copy-Item packaging\licenses\ggml.txt (Join-Path $dir 'licenses\ggml.txt')
    Copy-Item packaging\licenses\ggml.txt (Join-Path $dir 'licenses\llama.cpp.txt')
    Copy-Item packaging\licenses\libffi.txt (Join-Path $dir 'licenses\libffi.txt')
    $openmp = Join-Path $Folder 'licenses\llvm-openmp.txt'
    if (-not (Test-Path $openmp)) { throw "$openmp does not exist" }
    Copy-Item $openmp (Join-Path $dir 'licenses\llvm-openmp.txt')

    $readme = @"
# sayso-cleanup $version (windows-x64)

The cleanup runtime of Sayso: it runs a local cleanup model (GGUF) with
llama.cpp and speaks the cleanup protocol v1 (NDJSON on stdin and stdout).
The SDKs start it as ``@justsayso/cleanup`` or from this folder.

``````
& { '{"v":1,"id":"1","type":"hello"}'; Start-Sleep 2 } | .\sayso-cleanup.exe
``````

The hello reply says ``"libraries":"found"``: the llama.cpp DLLs are in this
folder. Keep the DLLs next to ``sayso-cleanup.exe``. They include the Visual C++
runtime, so the computer needs no other install. License: GPL-3.0-only
(LICENSE); the licenses of llama.cpp, ggml, libffi, and the LLVM OpenMP runtime
are in licenses\.
"@
    [IO.File]::WriteAllText((Join-Path $dir 'README.md'), $readme.Replace("`r`n", "`n") + "`n", (New-Object Text.UTF8Encoding $false))

    # The copy must find its libraries in the folder, with no cache to fall back on.
    New-Item -ItemType Directory -Force $checkHome | Out-Null
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = Join-Path $dir 'sayso-cleanup.exe'
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables['LOCALAPPDATA'] = $checkHome
    $psi.EnvironmentVariables['USERPROFILE'] = $checkHome
    $psi.EnvironmentVariables['SAYSO_CLEANUP_LOG'] = 'off'
    $psi.EnvironmentVariables.Remove('SAYSO_LLAMA_LIB_DIR')
    $p = [Diagnostics.Process]::Start($psi)
    $p.StandardInput.WriteLine('{"v":1,"id":"1","type":"hello"}')
    $p.StandardInput.Flush()
    $line = $p.StandardOutput.ReadLineAsync()
    if (-not $line.Wait(15000)) { $p.Kill(); throw 'the packed runtime did not answer hello in 15 s' }
    $reply = $line.Result
    $p.StandardInput.Close()
    if (-not $p.WaitForExit(15000)) { $p.Kill() }
    Write-Host $reply
    if ($reply -notlike '*"type":"hello"*' -or $reply -notlike '*"libraries":"found"*') { throw 'the packed runtime does not find its libraries' }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = Join-Path $Out $file
    if (Test-Path $zip) { Remove-Item -Force $zip }
    [IO.Compression.ZipFile]::CreateFromDirectory($dir, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)

    $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    Set-Content -Encoding ascii -NoNewline -Path "$zip.sha256" -Value "$hash  $file`n"
    Write-Host "==> cleanup file: $zip"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $stage, $checkHome
}
