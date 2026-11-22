# Development run on Windows: build the Go engine, then go run the app.
#
#   scripts\dev.cmd [app flags]       for example: scripts\dev.cmd --onboarding --dark
#   powershell -ExecutionPolicy Bypass -File scripts\dev.ps1 [app flags]
#   ./scripts/dev.sh [app flags]      from Git Bash, which hands over to this script
#
# XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_CACHE_HOME, SAYSO_FAKE_MIC,
# SAYSO_ENGINE_PATH, and SAYSO_CLEANUP_PATH pass through to the app. Press Ctrl+C to quit Sayso.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:CGO_ENABLED = '0'

function Invoke-Checked([string]$what, [scriptblock]$block) {
    Write-Host "==> $what"
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

if (-not $env:SAYSO_ENGINE_PATH) {
    New-Item -ItemType Directory -Force 'build\dev' | Out-Null
    Invoke-Checked 'engine (go)' { go build -o build\dev\sayso-engine.exe .\cmd\sayso-engine }
    $env:SAYSO_ENGINE_PATH = Join-Path $root 'build\dev\sayso-engine.exe'
}

Write-Host "==> go run .\cmd\sayso $args"
go run .\cmd\sayso @args
exit $LASTEXITCODE
