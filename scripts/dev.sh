#!/usr/bin/env bash
# Development run of the Go/MyGo app.
#
#   scripts/dev.sh [app flags]            go run ./cmd/sayso with the Swift engine (macOS)
#   scripts/dev.sh --bundle [app flags]   build a debug Sayso.app (signed) and open it
#
# `go run` starts Sayso as a child of the terminal, so macOS uses the terminal's
# permissions. Use --bundle to get the prompts and grants of dev.sayso.Sayso.
#
# XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_CACHE_HOME, SAYSO_FAKE_MIC,
# SAYSO_ENGINE_PATH, SAYSO_CLEANUP_PATH, SAYSO_LLAMA_LIB_DIR, SAYSO_UPDATE_URL,
# and SAYSO_UPDATE_KEY pass through to the app. Without SAYSO_CLEANUP_PATH, the
# script builds the cleanup runtime and points it at the pinned llama.cpp
# libraries in build/llama-libs. Press Ctrl+C to quit Sayso.
set -euo pipefail
cd "$(dirname "$0")/.."

# Git Bash on Windows: the Windows script does the same steps.
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 "$@" ;;
esac

export CGO_ENABLED=0

if [[ "${1:-}" == "--bundle" ]]; then
  shift
  [[ "$(uname -s)" == Darwin ]] || { echo "--bundle is for macOS" >&2; exit 1; }
  scripts/bundle.sh --debug
  app="$PWD/build/Sayso.app"
  exe="$app/Contents/MacOS/Sayso"
  pkill -f "$exe" 2>/dev/null || true
  trap 'pkill -f "$exe" || true; exit 0' INT TERM
  open -n "$app" --args "$@"
  echo "==> started $app (Ctrl+C quits Sayso)"
  sleep 2
  while pgrep -f "$exe" >/dev/null; do sleep 1; done
  exit 0
fi

if [[ "$(uname -s)" == Darwin && -z "${SAYSO_ENGINE_PATH:-}" ]]; then
  echo "==> engine (swift, release)"
  (cd native/macos/SaysoEngine && swift build -c release 2>&1 | tail -1)
  export SAYSO_ENGINE_PATH="$PWD/native/macos/SaysoEngine/.build/release/SaysoEngine"
else
  # Windows and Linux: the Go engine, next to a dev build.
  if [[ -z "${SAYSO_ENGINE_PATH:-}" ]]; then
    echo "==> engine (go)"
    mkdir -p build/dev
    go build -o build/dev/sayso-engine ./cmd/sayso-engine
    export SAYSO_ENGINE_PATH="$PWD/build/dev/sayso-engine"
  fi
fi

if [[ -z "${SAYSO_CLEANUP_PATH:-}" ]]; then
  echo "==> cleanup runtime (go)"
  mkdir -p build/dev
  go build -o build/dev/sayso-cleanup ./cmd/sayso-cleanup
  export SAYSO_CLEANUP_PATH="$PWD/build/dev/sayso-cleanup"
  if [[ -z "${SAYSO_LLAMA_LIB_DIR:-}" ]]; then
    SAYSO_LLAMA_LIB_DIR=$(go run ./scripts/llama-libs fetch -out build/llama-libs)
    export SAYSO_LLAMA_LIB_DIR
  fi
fi

echo "==> go run ./cmd/sayso $*"
exec go run ./cmd/sayso "$@"
