#!/bin/sh

set -eu

SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
TARGET="aarch64-apple-darwin"
SOURCE="$REPOSITORY_ROOT/target/$TARGET/release/zeta-computation-runner"
DESTINATION="$REPOSITORY_ROOT/src-tauri/binaries/zeta-computation-runner-$TARGET"
BROKER_DESTINATION="$REPOSITORY_ROOT/src-tauri/binaries/zeta-computation-broker-$TARGET"
XPC_SOURCE="$REPOSITORY_ROOT/src-tauri/computation-xpc"
SERVICE_STAGE="$REPOSITORY_ROOT/src-tauri/computation-service/com.flowzed.zeta.computation-service.xpc"
SERVICE_MACOS="$SERVICE_STAGE/Contents/MacOS"
SERVICE_EXECUTABLE="$SERVICE_MACOS/zeta-computation-service"
SERVICE_RUNNER="$SERVICE_MACOS/zeta-computation-runner"
RUNTIME_LOCK="$REPOSITORY_ROOT/src-tauri/computation-runtime.lock"
DIGEST_MANIFEST="$DESTINATION.sha256"
BROKER_DIGEST_MANIFEST="$BROKER_DESTINATION.sha256"
SERVICE_MANIFEST="$REPOSITORY_ROOT/src-tauri/computation-service/payload.sha256"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "SCRIPT-002 runner builds require Apple Silicon macOS." >&2
  exit 1
fi

for required in \
  'target=aarch64-apple-darwin' \
  'transport=xpc-service-v1' \
  'contract_version=zeta-contract-1' \
  'runtime_version=quickjs-ng-0.15.1+rquickjs-0.12.2+zeta-contract-1' \
  'rquickjs_version=0.12.2' \
  'max_child_processes=1'; do
  if ! grep -Fqx "$required" "$RUNTIME_LOCK"; then
    echo "The computation runtime lock is missing a reviewed policy value." >&2
    exit 1
  fi
done

verify_locked_file() {
  LOCK_KEY="$1"
  LOCKED_FILE="$2"
  EXPECTED=$(sed -n "s/^$LOCK_KEY=//p" "$RUNTIME_LOCK")
  ACTUAL=$(shasum -a 256 "$LOCKED_FILE" | awk '{print $1}')
  if [ "$EXPECTED" != "$ACTUAL" ]; then
    echo "A computation XPC source or entitlement differs from its reviewed lock." >&2
    exit 1
  fi
}

verify_locked_file broker_entitlements_sha256 "$REPOSITORY_ROOT/src-tauri/ComputationBroker.entitlements.plist"
verify_locked_file service_entitlements_sha256 "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist"
verify_locked_file runner_entitlements_sha256 "$REPOSITORY_ROOT/src-tauri/ComputationRunner.entitlements.plist"
verify_locked_file broker_source_sha256 "$XPC_SOURCE/broker.c"
verify_locked_file service_source_sha256 "$XPC_SOURCE/service.c"
verify_locked_file worker_source_sha256 "$XPC_SOURCE/worker.c"
verify_locked_file worker_header_sha256 "$XPC_SOURCE/worker.h"
verify_locked_file containment_probe_sha256 "$XPC_SOURCE/containment_probe.c"
verify_locked_file service_info_sha256 "$XPC_SOURCE/Info.plist"

cargo build \
  --locked \
  --release \
  --target "$TARGET" \
  --package computation-runtime \
  --bin zeta-computation-runner \
  --manifest-path "$REPOSITORY_ROOT/Cargo.toml"

install -m 0755 "$SOURCE" "$DESTINATION"
SDK_ROOT=$(xcrun --sdk macosx --show-sdk-path)
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  "$XPC_SOURCE/broker.c" -o "$BROKER_DESTINATION"
mkdir -p "$SERVICE_MACOS"
install -m 0755 "$SOURCE" "$SERVICE_RUNNER"
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  "$XPC_SOURCE/service.c" "$XPC_SOURCE/worker.c" -o "$SERVICE_EXECUTABLE"
install -m 0644 "$XPC_SOURCE/Info.plist" "$SERVICE_STAGE/Contents/Info.plist"
file "$DESTINATION"
for executable in "$DESTINATION" "$BROKER_DESTINATION" "$SERVICE_EXECUTABLE" "$SERVICE_RUNNER"; do
  if [ "$(lipo -archs "$executable")" != "arm64" ]; then
    echo "A computation executable is not an arm64-only Mach-O file." >&2
    exit 1
  fi
  if otool -L "$executable" | tail -n +2 | grep -q '@rpath\|/opt/homebrew\|/usr/local'; then
    echo "A computation executable has a non-system dynamic dependency." >&2
    exit 1
  fi
done

RUNNER_SHA256=$(shasum -a 256 "$DESTINATION" | awk '{print $1}')
printf '%s  %s\n' "$RUNNER_SHA256" "$(basename "$DESTINATION")" > "$DIGEST_MANIFEST"
chmod 0644 "$DIGEST_MANIFEST"
BROKER_SHA256=$(shasum -a 256 "$BROKER_DESTINATION" | awk '{print $1}')
printf '%s  %s\n' "$BROKER_SHA256" "$(basename "$BROKER_DESTINATION")" > \
  "$BROKER_DIGEST_MANIFEST"
chmod 0644 "$BROKER_DIGEST_MANIFEST"
(
  cd "$REPOSITORY_ROOT/src-tauri/computation-service"
  find com.flowzed.zeta.computation-service.xpc -type f -print | LC_ALL=C sort | \
    while IFS= read -r path; do
      shasum -a 256 "$path"
    done
) > "$SERVICE_MANIFEST"
chmod 0644 "$SERVICE_MANIFEST"
echo "SHA-256: $RUNNER_SHA256"
echo "Broker SHA-256: $BROKER_SHA256"
