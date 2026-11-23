#!/usr/bin/env bash
# Builds the Sayso Android app with the native typing core, from a clean
# checkout, in one go:
#
#   1. libsayso_core.so for arm64-v8a and x86_64 (build-core-android.sh)
#   2. checks models/en-US.sslm against the sha256 in DATA_SOURCES.md and opens it
#   3. the core and the JNI glue for this computer, so the JVM tests drive the real core
#   4. the sherpa-onnx Android AAR for local speech recognition, checked against
#      its pinned sha256 (fetch-sherpa-onnx.sh)
#   5. android/local.properties, if missing
#   6. ./gradlew testDebugUnitTest assembleDebug with the native engine
#   7. checks the APK holds both ABIs and no others, the JNI library, sherpa-onnx,
#      the model stored uncompressed, and no speech model weights, and prints its
#      path and size
#
#   ANDROID_HOME      SDK (default ~/Library/Android/sdk, then ~/Android/Sdk)
#   ANDROID_NDK_HOME  NDK (default: the version app/build.gradle.kts pins, under the SDK)
#   JAVA_HOME         JDK 21 (default: the java on PATH)
set -euo pipefail
cd "$(dirname "$0")/.."
mobile="$PWD"

die() { echo "build-android: $*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" ]]; then
  for d in "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do [[ -d "$d" ]] && sdk="$d" && break; done
fi
[[ -d "$sdk" ]] || die "no Android SDK found; set ANDROID_HOME"
ndk_version="$(sed -n 's/^ *ndkVersion = "\(.*\)"/\1/p' android/app/build.gradle.kts)"
if [[ -z "${ANDROID_NDK_HOME:-}" && -d "$sdk/ndk/$ndk_version" ]]; then
  export ANDROID_NDK_HOME="$sdk/ndk/$ndk_version"
fi

if [[ -z "${JAVA_HOME:-}" ]]; then
  javac_path="$(command -v javac || true)"
  [[ -n "$javac_path" ]] || die "no JDK found; set JAVA_HOME"
  JAVA_HOME="$(cd "$(dirname "$(readlink -f "$javac_path")")/.." && pwd)"
  export JAVA_HOME
fi

step "libsayso_core.so for Android"
./scripts/build-core-android.sh

step "model"
model=models/en-US.sslm
want="$(awk '/committed model/ { f = 1 } f && match($0, /[0-9a-f]{64}/) { print substr($0, RSTART, 64); exit }' tools/sslmgen/DATA_SOURCES.md)"
[[ -n "$want" ]] || die "no model checksum in tools/sslmgen/DATA_SOURCES.md"
if command -v sha256sum >/dev/null; then
  got="$(sha256sum "$model" | awk '{print $1}')"
else
  got="$(shasum -a 256 "$model" | awk '{print $1}')"
fi
[[ "$got" == "$want" ]] || die "$model sha256 $got, DATA_SOURCES.md says $want"
echo "sha256 $got (matches DATA_SOURCES.md)"
go run ./tools/sslminfo -model "$model"

step "host core and JNI glue for the JVM tests"
host="$mobile/build/host-jni"
mkdir -p "$host"
case "$(go env GOOS)" in
  darwin) ext=dylib jni_os=darwin core_ld="" jni_ld="-Wl,-rpath,@loader_path" ;;
  linux) ext=so jni_os=linux core_ld="-extldflags=-Wl,-soname,libsayso_core.so" jni_ld="-Wl,-rpath,\$ORIGIN" ;;
  *) die "host JNI build supports macOS and Linux" ;;
esac
CGO_ENABLED=1 go build -buildmode=c-shared -ldflags="$core_ld" -o "$host/libsayso_core.$ext" ./core/capi
rm -f "$host/libsayso_core.h"
if [[ "$ext" == dylib ]]; then
  install_name_tool -id @rpath/libsayso_core.dylib "$host/libsayso_core.dylib"
fi
cc -shared -fPIC -O2 -Wall -Wextra -Werror -I core/capi -I "$JAVA_HOME/include" -I "$JAVA_HOME/include/$jni_os" \
  android/app/src/main/cpp/sayso_jni.c -L "$host" -lsayso_core "$jni_ld" -o "$host/libsayso_jni.$ext"
ls -l "$host"/libsayso_*."$ext"

step "sherpa-onnx for Android"
sherpa_aar="$(./scripts/fetch-sherpa-onnx.sh)"
echo "$sherpa_aar (sha256 matches the pin)"

step "gradle"
cd android
if [[ ! -f local.properties ]]; then
  echo "sdk.dir=$sdk" > local.properties
  echo "wrote android/local.properties (sdk.dir=$sdk)"
fi
./gradlew testDebugUnitTest assembleDebug -PsaysoNative=true -PsaysoHostLibDir="$host" -PsaysoSherpaAar="$sherpa_aar"

step "APK"
apk="$PWD/app/build/outputs/apk/debug/app-debug.apk"
[[ -f "$apk" ]] || die "no APK at $apk"
listing="$(unzip -v "$apk")"
for f in lib/arm64-v8a/libsayso_core.so lib/x86_64/libsayso_core.so lib/arm64-v8a/libsayso_jni.so lib/x86_64/libsayso_jni.so \
  lib/arm64-v8a/libsherpa-onnx-jni.so lib/x86_64/libsherpa-onnx-jni.so; do
  grep -q " $f\$" <<<"$listing" || die "APK lacks $f"
done
if grep -E ' lib/' <<<"$listing" | grep -vE ' lib/(arm64-v8a|x86_64)/'; then die "APK holds an ABI other than arm64-v8a and x86_64"; fi
if grep -E '\.(onnx|ort|gguf)$' <<<"$listing"; then die "APK holds model weights; speech models are downloaded, not bundled"; fi
model_line="$(grep ' assets/en-US.sslm$' <<<"$listing")" || die "APK lacks assets/en-US.sslm"
awk '{ if ($2 != "Stored") exit 1 }' <<<"$model_line" || die "assets/en-US.sslm is compressed in the APK"
grep -E ' (lib/.*/libsayso_(core|jni)\.so|lib/.*/libsherpa-onnx-jni\.so|assets/en-US\.sslm)$' <<<"$listing"
tests="$(find app/build/test-results/testDebugUnitTest -name '*.xml' -exec grep -ho 'tests="[0-9]*"' {} + | tr -dc '0-9\n' | paste -sd+ - | bc)"
native_tests="$(grep -o 'tests="[0-9]*" skipped="[0-9]*"' app/build/test-results/testDebugUnitTest/TEST-app.sayso.mobile.engine.NativeTypingEngineTest.xml)"
echo "JVM tests: $tests (NativeTypingEngineTest: $native_tests)"
echo "APK: $apk ($(wc -c <"$apk" | tr -d ' ') bytes, $(du -h "$apk" | cut -f1))"
