#!/usr/bin/env bash
# Builds the llama-server the desktop app bundles, from one pinned llama.cpp commit.
# The result is a single self-contained executable, so the app runs a model right
# after install: macOS builds embed the Metal shaders; Windows builds are CPU-only
# (AVX2) with the C runtime linked statically.
#
# Usage: scripts/build-bundled-llama-server.sh <rust-target-triple>
set -euo pipefail

LLAMA_CPP_TAG="v0.8.0"
LLAMA_CPP_COMMIT="8642541b255bfb5dc39cd351a178be09013a55d6"

target="${1:?usage: build-bundled-llama-server.sh <aarch64-apple-darwin|x86_64-apple-darwin|x86_64-pc-windows-msvc>}"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  x86_64-pc-windows-msvc) arch=x64 ;;
  *) echo "unsupported target: $target" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$root/target/bundled-llama/$target"
out="$root/apps/desktop/src-tauri/binaries/llama-server-$target"

rm -rf "$work"
mkdir -p "$work" "$(dirname "$out")"
git -C "$work" init -q src
git -C "$work/src" fetch -q --depth 1 https://github.com/ggml-org/llama.cpp.git "$LLAMA_CPP_COMMIT"
git -C "$work/src" -c advice.detachedHead=false checkout -q FETCH_HEAD
actual="$(git -C "$work/src" rev-parse HEAD)"
if [ "$actual" != "$LLAMA_CPP_COMMIT" ]; then
  echo "llama.cpp $LLAMA_CPP_TAG resolved to $actual, expected $LLAMA_CPP_COMMIT" >&2
  exit 1
fi

common=(
  -DBUILD_SHARED_LIBS=OFF
  -DGGML_NATIVE=OFF
  -DLLAMA_CURL=OFF
  -DLLAMA_OPENSSL=OFF
  -DLLAMA_BUILD_TESTS=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF
  -DLLAMA_BUILD_TOOLS=ON
  -DLLAMA_BUILD_SERVER=ON
)
fail() { tail -40 "$work/build.log" >&2; exit 1; }
if [ "$arch" = x64 ]; then
  cmake -S "$work/src" -B "$work/build" -G "Visual Studio 17 2022" -A x64 "${common[@]}" \
    -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded \
    -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON \
    >"$work/build.log" 2>&1 || fail
  cmake --build "$work/build" --config Release --target llama-server --parallel >>"$work/build.log" 2>&1 || fail
  binary="$work/build/bin/Release/llama-server.exe"
  test -f "$binary"
  "$binary" --version 2>&1 | grep -q 'version: '
  cp "$binary" "$out.exe"
  echo "$out.exe"
  exit 0
fi

cmake -S "$work/src" -B "$work/build" "${common[@]}" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES="$arch" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=12.0 \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_BLAS=OFF \
  >"$work/build.log" 2>&1 || fail
cmake --build "$work/build" --config Release --target llama-server -j "$(sysctl -n hw.ncpu)" >>"$work/build.log" 2>&1 || fail

binary="$work/build/bin/llama-server"
test -x "$binary"
if otool -L "$binary" | tail -n +2 | grep -v -E '^\s*(/usr/lib/|/System/Library/)'; then
  echo "llama-server links a non-system library; the bundle would not be self-contained" >&2
  exit 1
fi
test "$(lipo -archs "$binary")" = "$arch"
cp "$binary" "$out"
echo "$out"
