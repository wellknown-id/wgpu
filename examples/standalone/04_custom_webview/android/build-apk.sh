#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../../.." && pwd)"

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "${ANDROID_HOME}" ]]; then
  echo "ANDROID_HOME or ANDROID_SDK_ROOT must be set" >&2
  exit 1
fi

BUILD_TOOLS_VERSION="${BUILD_TOOLS_VERSION:-36.1.0}"
TARGET_TRIPLE="${TARGET_TRIPLE:-aarch64-linux-android}"
ANDROID_API="${ANDROID_API:-34}"
MIN_ANDROID_API="${MIN_ANDROID_API:-28}"
PROFILE="${PROFILE:-debug}"
NDK_HOME="${NDK_HOME:-$(find "$ANDROID_HOME/ndk" -maxdepth 1 -mindepth 1 -type d | sort | tail -n 1)}"

case "$PROFILE" in
  debug) PROFILE_FLAG=() ;;
  release) PROFILE_FLAG=(--release) ;;
  *)
    echo "PROFILE must be debug or release" >&2
    exit 1
    ;;
esac

AAPT2="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION/aapt2"
D8="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION/d8"
ZIPALIGN="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION/zipalign"
APKSIGNER="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION/apksigner"
ANDROID_JAR="$ANDROID_HOME/platforms/android-$ANDROID_API/android.jar"
TOOLCHAIN_BIN="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
CLANG="$TOOLCHAIN_BIN/aarch64-linux-android${MIN_ANDROID_API}-clang"
CLANGXX="$TOOLCHAIN_BIN/aarch64-linux-android${MIN_ANDROID_API}-clang++"
LLVM_AR="$TOOLCHAIN_BIN/llvm-ar"
LLVM_RANLIB="$TOOLCHAIN_BIN/llvm-ranlib"

for tool in "$AAPT2" "$D8" "$ZIPALIGN" "$APKSIGNER" "$ANDROID_JAR" "$CLANG" "$CLANGXX" "$LLVM_AR" "$LLVM_RANLIB"; do
  if [[ ! -e "$tool" ]]; then
    echo "Missing Android tool or platform file: $tool" >&2
    exit 1
  fi
done

pushd "$CRATE_DIR" >/dev/null
CC="$CLANG" \
CXX="$CLANGXX" \
AR="$LLVM_AR" \
RANLIB="$LLVM_RANLIB" \
TARGET_CC="$CLANG" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CLANG" \
cargo build --lib -F xr --target "$TARGET_TRIPLE" "${PROFILE_FLAG[@]}"
popd >/dev/null

WORK_DIR="$REPO_ROOT/target/$PROFILE/custom_webview_hybrid_apk"
OUTPUT_APK="$REPO_ROOT/target/$PROFILE/apk/wgpu_example_04_custom_webview_hybrid.apk"
LIB_DIR="$REPO_ROOT/target/$TARGET_TRIPLE/$PROFILE"

rm -rf "$WORK_DIR"
mkdir -p \
  "$WORK_DIR/classes" \
  "$WORK_DIR/dex" \
  "$WORK_DIR/lib/arm64-v8a" \
  "$(dirname "$OUTPUT_APK")"

mapfile -t JAVA_SOURCES < <(find "$SCRIPT_DIR/src" -name '*.java' | sort)
javac --release 8 -cp "$ANDROID_JAR" -d "$WORK_DIR/classes" "${JAVA_SOURCES[@]}"

mapfile -t CLASS_FILES < <(find "$WORK_DIR/classes" -name '*.class' | sort)
"$D8" --min-api "$MIN_ANDROID_API" --lib "$ANDROID_JAR" --output "$WORK_DIR/dex" "${CLASS_FILES[@]}"

cp "$LIB_DIR/libwgpu_example_04_custom_webview.so" "$WORK_DIR/lib/arm64-v8a/"
cp "$CRATE_DIR/jniLibs/arm64-v8a/libopenxr_loader.so" "$WORK_DIR/lib/arm64-v8a/"

"$AAPT2" link \
  --manifest "$SCRIPT_DIR/AndroidManifest.xml" \
  -I "$ANDROID_JAR" \
  -o "$WORK_DIR/base.apk"

python - "$WORK_DIR" <<'PY'
import os
import sys
import zipfile

work_dir = sys.argv[1]
base_apk = os.path.join(work_dir, "base.apk")
merged_apk = os.path.join(work_dir, "merged-unaligned.apk")

with zipfile.ZipFile(merged_apk, "w") as out:
    with zipfile.ZipFile(base_apk, "r") as base:
        for info in base.infolist():
            out.writestr(info, base.read(info.filename))

    for filename in sorted(os.listdir(os.path.join(work_dir, "dex"))):
        path = os.path.join(work_dir, "dex", filename)
        if os.path.isfile(path):
            out.write(path, filename, compress_type=zipfile.ZIP_STORED)

    for root, _, files in os.walk(os.path.join(work_dir, "lib")):
        for filename in sorted(files):
            path = os.path.join(root, filename)
            arcname = os.path.relpath(path, work_dir)
            out.write(path, arcname, compress_type=zipfile.ZIP_STORED)
PY

"$ZIPALIGN" -f 4 "$WORK_DIR/merged-unaligned.apk" "$OUTPUT_APK"

KEYSTORE_PATH="${KEYSTORE_PATH:-$HOME/.android/debug.keystore}"
KEYSTORE_PASSWORD="${KEYSTORE_PASSWORD:-android}"
"$APKSIGNER" sign --ks "$KEYSTORE_PATH" --ks-pass "pass:$KEYSTORE_PASSWORD" "$OUTPUT_APK"

echo "$OUTPUT_APK"
