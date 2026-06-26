#!/usr/bin/env bash
# Builds llama.cpp (ggml + Metal + RPC) as an iOS xcframework pinned to the host's tag.
# Re-run whenever the cluster's llama.cpp version moves. Keep TAG == the host's build.
set -euo pipefail

TAG="${LLAMA_TAG:-b9789}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/.llama-ios-build"
OUT="$ROOT/ios/AirpcezWorker/Frameworks"
GEN="$ROOT/ios/AirpcezWorker/Generated"

rm -rf "$WORK" && mkdir -p "$WORK" "$OUT" "$GEN"
git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/llama.cpp "$WORK/llama.cpp"
cd "$WORK/llama.cpp"

# Common CMake flags: enable Metal + RPC, disable everything that won't link on iOS.
# The OFF set mirrors llama.cpp's own build-xcframework.sh (the canonical reference);
# we additionally enable GGML_RPC (upstream's script does not). Every executable-producing
# target MUST be OFF — they need an iOS bundle id / code signing and break the static-lib
# build. APP (the unified "llama" binary) and SERVER default ON for a standalone build, so
# disabling EXAMPLES/TOOLS/TESTS alone is not enough — APP and SERVER must be set explicitly.
common_flags=(
  -DGGML_METAL=ON -DGGML_RPC=ON
  -DGGML_METAL_EMBED_LIBRARY=ON          # bake the .metallib into the binary (no bundle path)
  -DGGML_METAL_USE_BF16=ON               # bf16 Metal kernels (matches upstream xcframework build)
  -DLLAMA_BUILD_APP=OFF -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_COMMON=OFF
  -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_TOOLS=OFF
  -DLLAMA_CURL=OFF
  -DBUILD_SHARED_LIBS=OFF
  -DCMAKE_BUILD_TYPE=Release
)

build_one () { # $1 = sysroot name, $2 = platform dir, $3 = arch
  cmake -B "build-$1" -G Xcode \
    -DCMAKE_SYSTEM_NAME=iOS \
    -DCMAKE_OSX_SYSROOT="$1" \
    -DCMAKE_OSX_ARCHITECTURES="$3" \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=17.0 \
    "${common_flags[@]}"
  cmake --build "build-$1" --config Release -- -quiet
}

build_one iphoneos device arm64
build_one iphonesimulator simulator arm64

# Collect the static libs (ggml*, llama) per slice into a single lib + headers.
package_slice () { # $1 = build dir, $2 = dest dir
  mkdir -p "$2/lib" "$2/include"
  find "$1" -name '*.a' -exec cp {} "$2/lib/" \;
  cp -R src/*.h include/*.h ggml/include/*.h "$2/include/" 2>/dev/null || true
  libtool -static -o "$2/libllama_full.a" "$2"/lib/*.a
}
package_slice "build-iphoneos" "$WORK/slice-device"
package_slice "build-iphonesimulator" "$WORK/slice-sim"

rm -rf "$OUT/llama.xcframework"
xcodebuild -create-xcframework \
  -library "$WORK/slice-device/libllama_full.a" -headers "$WORK/slice-device/include" \
  -library "$WORK/slice-sim/libllama_full.a"    -headers "$WORK/slice-sim/include" \
  -output "$OUT/llama.xcframework"

printf 'enum LlamaVersion { static let tag = "%s" }\n' "$TAG" > "$GEN/LlamaVersion.swift"
echo "Built llama.xcframework ($TAG) -> $OUT"
