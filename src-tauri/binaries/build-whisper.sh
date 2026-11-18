#!/bin/zsh
# Rebuilds whisper-cli for Juggl (src/microphone.rs): static whisper.cpp with Metal, the Metal library inside the
# binary, no Homebrew dependency. Tauri puts it in Juggl.app/Contents/MacOS (bundle.externalBin) and signs it.
#   build-whisper.sh [version]    -> whisper-cli-aarch64-apple-darwin, -x86_64-apple-darwin and -universal-apple-darwin
# Juggl's universal build (pubblica.sh) wants the -universal-apple-darwin file.
set -euo pipefail
VERSION=${1:-v1.9.5}
HERE=${0:A:h}
WORKDIR=$(mktemp -d)
git clone -q --depth 1 --branch "$VERSION" https://github.com/ggml-org/whisper.cpp "$WORKDIR/whisper.cpp"
cd "$WORKDIR/whisper.cpp"
for ARCH in arm64 x86_64; do
  nice -n 10 cmake -B build-$ARCH -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON \
    -DWHISPER_BUILD_TESTS=OFF -DWHISPER_SDL2=OFF -DGGML_NATIVE=OFF -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
    -DCMAKE_OSX_ARCHITECTURES=$ARCH >/dev/null
  nice -n 10 cmake --build build-$ARCH -j 3 --target whisper-cli >/dev/null
done
cp build-arm64/bin/whisper-cli "$HERE/whisper-cli-aarch64-apple-darwin"
cp build-x86_64/bin/whisper-cli "$HERE/whisper-cli-x86_64-apple-darwin"
strip -x "$HERE/whisper-cli-aarch64-apple-darwin" "$HERE/whisper-cli-x86_64-apple-darwin"
lipo -create -output "$HERE/whisper-cli-universal-apple-darwin" "$HERE/whisper-cli-aarch64-apple-darwin" "$HERE/whisper-cli-x86_64-apple-darwin"
rm -rf "$WORKDIR"
lipo -info "$HERE/whisper-cli-universal-apple-darwin"
