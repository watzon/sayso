#!/usr/bin/env bash
# Development run: build a debug bundle, start it with `open`, and show its log.
#
#   scripts/dev.sh [app flags]      for example: scripts/dev.sh --onboarding --dark
#
# `cargo run` starts Sayso as a child of the terminal, so macOS uses the
# terminal's permissions and never shows the microphone prompt. This script
# starts the signed bundle (dev.sayso.Sayso) through Launch Services instead,
# so permission prompts and grants belong to Sayso.
#
# XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_CACHE_HOME, RUST_LOG, SAYSO_FAKE_MIC,
# SAYSO_ENGINE_PATH, SAYSO_UPDATE_URL, and SAYSO_UPDATE_KEY pass through to the
# app. Press Ctrl+C to quit Sayso.
set -euo pipefail
cd "$(dirname "$0")/.."

# Git Bash on Windows: the Windows script does the same steps.
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 "$@" ;;
esac

scripts/bundle.sh --debug
app="$PWD/build/Sayso.app"
exe="$app/Contents/MacOS/Sayso"

if pgrep -f "$exe" >/dev/null; then
  echo "==> quitting the Sayso that is already running from build/"
  pkill -f "$exe" || true
  sleep 0.5
fi

if pgrep -fl 'target/(debug|release)/sayso( |$)' >/dev/null; then
  echo "warning: a Sayso started with cargo is still running. Quit it first, or both apps react to the hotkey." >&2
fi

env_args=()
for var in XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME RUST_LOG SAYSO_FAKE_MIC SAYSO_ENGINE_PATH SAYSO_UPDATE_URL SAYSO_UPDATE_KEY; do
  [[ -n "${!var:-}" ]] && env_args+=(--env "$var=${!var}")
done

cache="${XDG_CACHE_HOME:+$XDG_CACHE_HOME/sayso}"
log="${cache:-$HOME/Library/Caches/Sayso}/logs/sayso.log"
mkdir -p "$(dirname "$log")"
: > "$log"

trap 'pkill -f "$exe" || true; kill "${tail_pid:-}" 2>/dev/null || true; exit 0' INT TERM
echo "==> starting $app"
open -n ${env_args[@]+"${env_args[@]}"} "$app" --args "$@"
echo "==> log: $log (Ctrl+C quits Sayso)"
tail -n +1 -F "$log" &
tail_pid=$!
# Wait until the app quits, then stop the log.
sleep 2
while pgrep -f "$exe" >/dev/null; do sleep 1; done
kill "$tail_pid" 2>/dev/null || true
echo "==> Sayso quit"
