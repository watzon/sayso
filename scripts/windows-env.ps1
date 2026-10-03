# Build tools for the speech engine on Windows. Dot-source it: . scripts\windows-env.ps1
#
# whisper.cpp builds with CMake, and its Rust bindings need libclang. This
# finds both where they are usually installed, so a plain PowerShell works
# without setting PATH by hand. Variables that are already set win.

# CMake and Ninja installed with `pip install --user cmake ninja`.
if (-not (Get-Command cmake.exe -ErrorAction SilentlyContinue)) {
    $pip = Get-ChildItem "$env:APPDATA\Python\Python*\Scripts\cmake.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pip) { $env:PATH = "$env:PATH;$($pip.DirectoryName)" }
}
if (-not (Get-Command cmake.exe -ErrorAction SilentlyContinue)) {
    Write-Warning 'CMake was not found. The speech engine needs it to build whisper.cpp (see native\portable\NOTES.md).'
}

# Ninja: the default Visual Studio generator fails on long checkout paths.
# Use Ninja whenever it is there, so every build uses the same generator.
if (-not $env:CMAKE_GENERATOR -and (Get-Command ninja.exe -ErrorAction SilentlyContinue)) {
    $env:CMAKE_GENERATOR = 'Ninja'
}

# libclang: LLVM, else the clang of the Swift toolchain.
if (-not $env:LIBCLANG_PATH) {
    $candidates = @("$env:ProgramFiles\LLVM\bin") +
        @(Get-ChildItem "$env:LOCALAPPDATA\Programs\Swift\Toolchains\*\usr\bin" -Directory -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
    $found = $candidates | Where-Object { Test-Path (Join-Path $_ 'libclang.dll') } | Select-Object -First 1
    if ($found) { $env:LIBCLANG_PATH = $found }
}
