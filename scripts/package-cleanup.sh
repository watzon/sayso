#!/usr/bin/env bash
# Make the cleanup runtime release file for macOS or Linux: a tarball of
# sayso-cleanup with its llama.cpp libraries, its licenses, and its checksum.
#
#   scripts/package-cleanup.sh [--out <folder>] [--require-signed] <app or library folder>
#
#   dist/sayso-cleanup-<version>-<macos-arm64|linux-x86_64|linux-arm64>.tar.gz
#   dist/sayso-cleanup-<version>-<...>.tar.gz.sha256
#
# macOS: name Sayso.app. The file holds sayso-cleanup-<version>/bin/sayso-cleanup
# and Frameworks/llama/, the layout of Contents/ in the app: the runtime finds
# its libraries in ../Frameworks/llama (internal/llamalib BundledDir), and
# its libffi at @executable_path/../Frameworks/llama/libffi.8.dylib. The files
# are the signed ones of the app.
# Linux: name the bin/llama folder of the tarball of scripts/bundle-linux.sh.
# The file holds sayso-cleanup-<version>/sayso-cleanup with the libraries
# beside it, as there. libffi comes from the system (libffi8).
#
# The script never builds the runtime. Before it packs, it runs hello on the
# copy with an empty cache folder, and stops when the runtime does not find
# its libraries in the file: a user must never wait for their download.
# --require-signed (macOS) stops the script when a file has no valid
# Developer ID signature. The Windows file is made by scripts/package-cleanup.ps1.
set -euo pipefail
cd "$(dirname "$0")/.."
usage() { echo "usage: $0 [--out <folder>] [--require-signed] <app or library folder>" >&2; exit 2; }
out=dist; require_signed=0; from=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) [[ $# -ge 2 ]] || usage; out=$2; shift 2 ;;
    --require-signed) require_signed=1; shift ;;
    -*) usage ;;
    *) [[ -z "$from" ]] || usage; from=$1; shift ;;
  esac
done
[[ -n "$from" && -d "$from" ]] || usage

version=$(scripts/version.sh)
case "$(uname -s)" in
  Darwin) system=macos ;;
  Linux) system=linux ;;
  *) echo "error: this script runs on macOS and Linux. Use scripts/package-cleanup.ps1 on Windows." >&2; exit 1 ;;
esac
[[ $require_signed -eq 1 && $system != macos ]] && { echo "error: --require-signed is for macOS" >&2; exit 2; }

name="sayso-cleanup-$version"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
dir="$stage/$name"
mkdir -p "$dir/licenses" "$out"

case "$system" in
  macos)
    runtime="$from/Contents/MacOS/sayso-cleanup"
    libs="$from/Contents/Frameworks/llama"
    [[ -x "$runtime" && -d "$libs" ]] || { echo "error: $from has no Contents/MacOS/sayso-cleanup and Contents/Frameworks/llama" >&2; exit 1; }
    case "$(lipo -archs "$runtime")" in
      arm64) arch=arm64 ;;
      x86_64) arch=x86_64 ;;
      *) echo "error: the runtime must have one architecture. It has: $(lipo -archs "$runtime")" >&2; exit 1 ;;
    esac
    mkdir -p "$dir/bin" "$dir/Frameworks/llama"
    install -m 755 "$runtime" "$dir/bin/sayso-cleanup"
    cp -a "$libs"/. "$dir/Frameworks/llama/"
    exe="$dir/bin/sayso-cleanup"
    if [[ $require_signed -eq 1 ]]; then
      for f in "$exe" "$dir"/Frameworks/llama/*.dylib; do
        codesign --verify --strict "$f"
        authority=$(codesign -dvv "$f" 2>&1 | sed -n 's/^Authority=//p' | head -1)
        [[ "$authority" == "Developer ID Application:"* ]] || { echo "error: $(basename "$f") has no Developer ID signature (authority: ${authority:-none})." >&2; exit 1; }
      done
      echo "==> signatures ok"
    else
      echo "warning: the files are not checked for a Developer ID signature." >&2
    fi
    install -m 644 packaging/licenses/libffi.txt "$dir/licenses/libffi.txt"
    ;;
  linux)
    [[ -x "$from/sayso-cleanup" ]] || { echo "error: $from has no sayso-cleanup" >&2; exit 1; }
    case "$(uname -m)" in
      arm64|aarch64) arch=arm64; machine=b700 ;;
      x86_64) arch=x86_64; machine=3e00 ;;
      *) echo "error: no cleanup file for the architecture $(uname -m)" >&2; exit 1 ;;
    esac
    found=$(od -An -tx1 -j18 -N2 "$from/sayso-cleanup" | tr -d ' \n')
    [[ "$found" == "$machine" ]] || { echo "error: the runtime is not an $arch ELF file (machine code $found)." >&2; exit 1; }
    cp -a "$from"/. "$dir/"
    exe="$dir/sayso-cleanup"
    deps=$(ldd "$dir"/lib*.so* 2>/dev/null || true)
    if grep -q 'not found' <<< "$deps"; then
      echo "$deps"
      echo "error: a llama.cpp library misses a library." >&2
      exit 1
    fi
    install -m 644 packaging/licenses/libgomp.txt "$dir/licenses/libgomp.txt"
    ;;
esac
platform="$system-$arch"
file="$name-$platform.tar.gz"
cp LICENSE "$dir/LICENSE"
# llama.cpp has the MIT text of ggml, byte for byte.
install -m 644 packaging/licenses/ggml.txt "$dir/licenses/ggml.txt"
install -m 644 packaging/licenses/ggml.txt "$dir/licenses/llama.cpp.txt"

cat > "$dir/README.md" <<EOF
# sayso-cleanup $version ($platform)

The cleanup runtime of Sayso: it runs a local cleanup model (GGUF) with
llama.cpp and speaks the cleanup protocol v1 (NDJSON on stdin and stdout).
The SDKs start it as \`@justsayso/cleanup\` or from this folder.

\`\`\`
(echo '{"v":1,"id":"1","type":"hello"}'; sleep 1) | ./$( [[ $system == macos ]] && echo bin/ )sayso-cleanup
\`\`\`

The hello reply says \`"libraries":"found"\`: the llama.cpp libraries are in
this folder. Keep the folder as it is. License: GPL-3.0-only (LICENSE); the
licenses of llama.cpp, ggml, and $( [[ $system == macos ]] && echo libffi || echo libgomp ) are in licenses/.
EOF

# The copy must find its libraries in the folder, with no cache to fall back on.
check_home=$(mktemp -d)
reply=$( (printf '{"v":1,"id":"1","type":"hello"}\n'; sleep 3) | HOME="$check_home" XDG_CACHE_HOME="$check_home/cache" SAYSO_CLEANUP_LOG=off "$exe" 2>/dev/null || true)
rm -rf "$check_home"
echo "$reply"
[[ "$reply" == *'"type":"hello"'* && "$reply" == *'"libraries":"found"'* ]] || { echo "error: the packed runtime does not find its libraries." >&2; exit 1; }

tar_owner=(--owner=0 --group=0 --numeric-owner)
[[ $system == macos ]] && tar_owner=(--uid 0 --gid 0 --numeric-owner)
rm -f "$out/$file" "$out/$file.sha256"
COPYFILE_DISABLE=1 tar -C "$stage" "${tar_owner[@]}" -czf "$out/$file" "$name"
if command -v sha256sum >/dev/null; then
  (cd "$out" && sha256sum "$file" > "$file.sha256")
else
  (cd "$out" && shasum -a 256 "$file" > "$file.sha256")
fi
echo "==> cleanup file:"
/bin/ls -lh "$out/$file" "$out/$file.sha256"
