#!/usr/bin/env bash
# Run the cleanup runtime of the app (cmd/sayso-cleanup with the llama.cpp
# libraries in bin/llama) on older x86_64 processors under QEMU. There is one
# build: ggml has a CPU library for each processor level (libggml-cpu-x64,
# -sse42, -sandybridge, -haswell, and newer) and loads the best one that the
# processor runs, so a processor without AVX2 works too (issue #37).
#
#   scripts/test-llama-cpus.sh <folder llama of the tarball> <model.gguf>
#
# Needs qemu-x86_64 (Debian and Ubuntu: qemu-user) and libffi.so.8. The model
# can be the one of scripts/tiny-gguf.py: a load and a short completion run
# the kernels.
set -euo pipefail
[[ $# -eq 2 ]] || { echo "usage: $0 <llama folder> <model.gguf>" >&2; exit 2; }
dir=$(realpath "$1") model=$(realpath "$2")
command -v qemu-x86_64 >/dev/null || { echo "qemu-x86_64 is missing (qemu-user)" >&2; exit 1; }

requests='{"v":1,"id":"1","type":"hello"}
{"v":1,"id":"2","type":"load","model_id":"tiny","path":"'"$model"'","n_ctx":512}
{"v":1,"id":"3","type":"complete","model_id":"tiny","prompt":"the cat sat on the mat","end":[],"max_tokens":8}'

failed=0
# qemu64 is x86-64 with no SSE 4.2. Nehalem has SSE 4.2 and no AVX.
# SandyBridge has AVX and no AVX2. Haswell has AVX2.
for cpu in qemu64 Nehalem SandyBridge Haswell; do
  log="$(mktemp)"
  status=0
  reply=$(printf '%s\n' "$requests" | (cd "$dir" && SAYSO_CLEANUP_LOG=debug timeout 300 qemu-x86_64 -cpu "$cpu" ./sayso-cleanup 2>"$log")) || status=$?
  variant=$(grep -oE 'libggml-cpu-[a-z0-9_]+\.so' "$log" | sort -u | tr '\n' ' ')
  if [[ $status -eq 0 && "$reply" == *'"type":"completion"'* ]]; then
    echo "ok: $cpu (${variant:-no CPU library in the log})"
  else
    echo "::error::the cleanup runtime did not complete on $cpu (status $status): $reply"
    tail -20 "$log"
    failed=1
  fi
  rm -f "$log"
done
exit $failed
