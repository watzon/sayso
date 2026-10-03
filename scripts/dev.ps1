# Development run on Windows: build the debug app and the engine, start Sayso, and show its log.
#
#   scripts\dev.cmd [app flags]       for example: scripts\dev.cmd --onboarding --dark
#   powershell -ExecutionPolicy Bypass -File scripts\dev.ps1 [app flags]
#   ./scripts/dev.sh [app flags]      from Git Bash, which hands over to this script
#
# The Windows counterpart of the macOS dev.sh. Windows ties no permission to
# the app, so Sayso runs straight from target\debug. XDG_CONFIG_HOME,
# XDG_DATA_HOME, XDG_CACHE_HOME, RUST_LOG, SAYSO_FAKE_MIC, and
# SAYSO_ENGINE_PATH pass through to the app. Press Ctrl+C to quit Sayso.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
. (Join-Path $PSScriptRoot 'windows-env.ps1')

function Invoke-Checked([string]$what, [scriptblock]$block) {
    Write-Host "==> $what"
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

Invoke-Checked 'engine (cargo, release)' { cargo build --release --manifest-path native\portable\Cargo.toml }
Invoke-Checked 'app (cargo, debug)' { cargo build -p sayso-app }

$exe = Join-Path $root 'target\debug\sayso.exe'

# A dev Sayso that still runs would only open its Hub for the new one.
$running = @(Get-Process -Name sayso -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe })
if ($running) {
    Write-Host '==> quitting the Sayso that is already running from target\debug'
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}
$others = @(Get-Process -Name sayso -ErrorAction SilentlyContinue | Where-Object { $_.Path -ne $exe })
if ($others) {
    Write-Warning "Another Sayso is running ($($others[0].Path)). Quit it first, or both apps react to the hotkey."
}

$cache = if ($env:XDG_CACHE_HOME) { Join-Path $env:XDG_CACHE_HOME 'sayso' } else { Join-Path $env:LOCALAPPDATA 'Sayso\Cache' }
$log = Join-Path $cache 'logs\sayso.log'
New-Item -ItemType Directory -Force (Split-Path $log) | Out-Null
Set-Content -Path $log -Value $null

Write-Host "==> starting $exe $args"
# Start-Process with an empty argument list fails, so pass the flags only when there are some.
# The debug build is a console program: it shares this console, and writes its log to the file.
$start = @{ FilePath = $exe; PassThru = $true; NoNewWindow = $true }
if ($args.Count -gt 0) { $start.ArgumentList = $args }
$app = Start-Process @start
Write-Host "==> log: $log (Ctrl+C quits Sayso)"

# Follow the log until Sayso quits. Ctrl+C runs the finally block, which quits Sayso.
try {
    $stream = [System.IO.File]::Open($log, 'Open', 'Read', 'ReadWrite, Delete')
    $reader = New-Object System.IO.StreamReader($stream)
    while (-not $app.HasExited) {
        $line = $reader.ReadLine()
        if ($null -ne $line) { Write-Host $line } else { Start-Sleep -Milliseconds 200 }
    }
    while ($null -ne ($line = $reader.ReadLine())) { Write-Host $line }
    $reader.Dispose()
    Write-Host '==> Sayso quit'
} finally {
    if (-not $app.HasExited) {
        Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
        Write-Host '==> Sayso quit'
    }
}
