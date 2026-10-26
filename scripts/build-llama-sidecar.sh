#!/bin/sh

set -eu

# Keep direct builds bounded too; public bootstrap passes its selected job count.
BUILD_JOBS=${1-3}
case "$BUILD_JOBS" in
  ''|*[!0-9]*) echo "Build jobs must be an integer from 1 to 16." >&2; exit 1 ;;
esac
if [ "$#" -gt 1 ] || [ "$BUILD_JOBS" -lt 1 ] || [ "$BUILD_JOBS" -gt 16 ]; then
  echo "Usage: $0 [jobs: 1–16]" >&2
  exit 1
fi

EXPECTED_COMMIT="704485942ab54bbbbf1f241b3550ffba35f5f37e"
SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
LLAMA_SOURCE="$REPOSITORY_ROOT/vendor/llama.cpp"
BUILD_DIRECTORY="$LLAMA_SOURCE/build-zeta"
DESTINATION_DIRECTORY="$REPOSITORY_ROOT/src-tauri/binaries"
DESTINATION="$DESTINATION_DIRECTORY/zeta-tokenizer-aarch64-apple-darwin"
HELPER_SOURCE="$REPOSITORY_ROOT/src-tauri/tokenizer-helper"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "MODEL-001 sidecar builds require Apple Silicon macOS." >&2
  exit 1
fi

if [ ! -d "$LLAMA_SOURCE/.git" ]; then
  echo "Missing vendored llama.cpp checkout at $LLAMA_SOURCE" >&2
  echo "Clone ggml-org/llama.cpp there and check out $EXPECTED_COMMIT." >&2
  exit 1
fi

ACTUAL_COMMIT=$(git -C "$LLAMA_SOURCE" rev-parse HEAD)
if [ "$ACTUAL_COMMIT" != "$EXPECTED_COMMIT" ]; then
  echo "Expected llama.cpp $EXPECTED_COMMIT, found $ACTUAL_COMMIT." >&2
  exit 1
fi

if [ -n "$(git -C "$LLAMA_SOURCE" status --porcelain --untracked-files=no)" ]; then
  echo "The vendored llama.cpp tracked files must be clean." >&2
  exit 1
fi

if [ -e "$LLAMA_SOURCE/tools/zeta-tokenizer" ]; then
  echo "Temporary tokenizer build directory already exists in vendored llama.cpp." >&2
  exit 1
fi
cleanup() {
  git -C "$LLAMA_SOURCE" restore -- tools/CMakeLists.txt
  rm -rf "$LLAMA_SOURCE/tools/zeta-tokenizer"
}
trap cleanup EXIT HUP INT TERM
cp -R "$HELPER_SOURCE" "$LLAMA_SOURCE/tools/zeta-tokenizer"
if ! grep -q 'add_subdirectory(zeta-tokenizer)' "$LLAMA_SOURCE/tools/CMakeLists.txt"; then
  printf '\nadd_subdirectory(zeta-tokenizer)\n' >> "$LLAMA_SOURCE/tools/CMakeLists.txt"
fi

cmake -S "$LLAMA_SOURCE" -B "$BUILD_DIRECTORY" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.3 \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_METAL=OFF \
  -DGGML_NATIVE=OFF \
  -DGGML_CCACHE=OFF \
  -DGGML_OPENMP=OFF \
  -DLLAMA_OPENSSL=OFF \
  -DLLAMA_SUBPROCESS=OFF \
  -DLLAMA_BUILD_UI=OFF \
  -DLLAMA_USE_PREBUILT_UI=OFF \
  -DLLAMA_BUILD_APP=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_TOOLS=ON \
  -DLLAMA_BUILD_SERVER=OFF

cmake --build "$BUILD_DIRECTORY" --config Release --target zeta-tokenizer --parallel "$BUILD_JOBS"
mkdir -p "$DESTINATION_DIRECTORY"
install -m 0755 "$BUILD_DIRECTORY/bin/zeta-tokenizer" "$DESTINATION"

file "$DESTINATION"
if otool -L "$DESTINATION" | tail -n +2 | grep -q '@rpath\|/opt/homebrew\|/usr/local'; then
  echo "The bundled helper has a non-system dynamic dependency." >&2
  exit 1
fi

echo "SHA-256: $(shasum -a 256 "$DESTINATION" | awk '{print $1}')"
