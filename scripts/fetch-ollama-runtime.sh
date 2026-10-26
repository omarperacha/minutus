#!/bin/sh

set -eu

VERSION="0.32.14"
EXPECTED_BYTES="153876009"
EXPECTED_SHA256="c7e8b91485943785bc6d295d96551e971ec94c6829d0d6b3500366942dc50cd1"
SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
DESTINATION="$REPOSITORY_ROOT/src-tauri/ollama-runtime"
TEMPORARY_DIRECTORY=$(mktemp -d)
ARCHIVE="$TEMPORARY_DIRECTORY/ollama-darwin.tgz"
PAYLOAD="$TEMPORARY_DIRECTORY/payload"

cleanup() {
  rm -rf -- "$TEMPORARY_DIRECTORY"
}
trap cleanup EXIT HUP INT TERM

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "QWEN-006.1 currently packages the qualified Apple Silicon runtime only." >&2
  exit 1
fi

ACCELERATION_ARTIFACT="${QWEN_ACCEL_ARTIFACT:-$REPOSITORY_ROOT/.cache/minutus-source/qwen-artifact}"
ACCELERATION_PIN="${QWEN_ACCEL_PIN:-$SCRIPT_DIRECTORY/qwen_accel_bundled.lock.json}"
python3 "$SCRIPT_DIRECTORY/stage_qwen_acceleration.py" \
  --artifact "$ACCELERATION_ARTIFACT" --pin "$ACCELERATION_PIN" --verify-only

curl --fail --location --http1.1 --retry 10 --retry-all-errors \
  --retry-delay 2 --continue-at - --output "$ARCHIVE" \
  "https://github.com/ollama/ollama/releases/download/v$VERSION/ollama-darwin.tgz"

ACTUAL_BYTES=$(stat -f '%z' "$ARCHIVE")
if [ "$ACTUAL_BYTES" != "$EXPECTED_BYTES" ]; then
  echo "Expected Ollama archive size $EXPECTED_BYTES, found $ACTUAL_BYTES." >&2
  exit 1
fi

ACTUAL_SHA256=$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')
if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
  echo "The Ollama archive failed its pinned SHA-256 check." >&2
  exit 1
fi

mkdir -p "$PAYLOAD"
tar -xzf "$ARCHIVE" -C "$PAYLOAD"
if [ ! -x "$PAYLOAD/ollama" ]; then
  echo "The verified archive does not contain its expected root Ollama executable." >&2
  exit 1
fi

find "$DESTINATION" -mindepth 1 -maxdepth 1 \
  ! -name README.md ! -name OLLAMA-LICENSE ! -name runtime.lock \
  -exec rm -rf -- {} +

cp -R "$PAYLOAD"/. "$DESTINATION"/
chmod 0755 "$DESTINATION/ollama"

(
  cd "$DESTINATION"
  find . \( -type f -o -type l \) ! -name payload.sha256 -exec shasum -a 256 {} + \
    | LC_ALL=C sort -k 2 > payload.sha256
)

"$DESTINATION/ollama" --version
ACCELERATION_PIN="${QWEN_ACCEL_PIN:-$SCRIPT_DIRECTORY/qwen_accel_bundled.lock.json}"
python3 "$SCRIPT_DIRECTORY/stage_qwen_acceleration.py" \
  --artifact "$ACCELERATION_ARTIFACT" --pin "$ACCELERATION_PIN"
echo "Prepared Ollama $VERSION runtime at $DESTINATION"
