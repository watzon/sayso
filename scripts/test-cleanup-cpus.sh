#!/usr/bin/env bash
# Run each x86_64 build of the cleanup runtime on the oldest processor that it
# is for, under QEMU, and check that the AVX2 build does stop on a processor
# without AVX2 (issue #37, scripts/build-cleanup.sh):
#
#   scripts/test-cleanup-cpus.sh <folder with the builds> <model.gguf>
#
# Needs qemu-x86_64 (Debian and Ubuntu: qemu-user). The model can be the one
# of scripts/tiny-gguf.py: a load and a short completion run the kernels.
set -euo pipefail
[[ $# -eq 2 ]] || { echo "usage: $0 <folder> <model.gguf>" >&2; exit 2; }
dir=$1 model=$(realpath "$2")
command -v qemu-x86_64 >/dev/null || { echo "qemu-x86_64 is missing (qemu-user)" >&2; exit 1; }

requests='{"v":1,"id":"1","type":"hello"}
{"v":1,"id":"2","type":"load","model_id":"tiny","path":"'"$model"'","n_ctx":512}
{"v":1,"id":"3","type":"complete","model_id":"tiny","prompt":"the cat sat on the mat","end":[],"max_tokens":8}'

# <build> <QEMU CPU model>: the status of the run, and its output in $reply.
run() {
  local status=0
  reply=$(printf '%s\n' "$requests" | timeout 300 qemu-x86_64 -cpu "$2" "$dir/$1" 2>/dev/null) || status=$?
  return $status
}

failed=0
# Nehalem has SSE 4.2 and no AVX. SandyBridge has AVX and no AVX2.
for pair in sayso-cleanup:Haswell sayso-cleanup-avx:SandyBridge sayso-cleanup-baseline:Nehalem sayso-cleanup-baseline:qemu64; do
  build=${pair%%:*} cpu=${pair#*:}
  if run "$build" "$cpu" && [[ "$reply" == *'"type":"completion"'* ]]; then
    echo "ok: $build on $cpu"
  else
    echo "::error::$build did not complete on $cpu: $reply"
    failed=1
  fi
done
# The check above means something only if a build for a newer processor stops.
status=0
run sayso-cleanup SandyBridge || status=$?
if [[ $status -eq 132 ]]; then
  echo "ok: sayso-cleanup stops with SIGILL on SandyBridge"
else
  echo "::error::sayso-cleanup did not stop with SIGILL on SandyBridge (status $status): $reply"
  failed=1
fi
exit $failed
