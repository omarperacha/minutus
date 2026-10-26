#!/bin/sh

set -eu

SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
STAGED_SERVICE="$REPOSITORY_ROOT/src-tauri/computation-service/com.flowzed.zeta.computation-service.xpc"
PROBE_ROOT="$REPOSITORY_ROOT/target/computation-xpc-probe"
PROBE="$PROBE_ROOT/ZetaComputationProbe.app"
PROBE_CONTENTS="$PROBE/Contents"
SERVICE_ID="com.flowzed.zeta.computation-probe-service.run$$"

if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]; then
  echo "The packaged XPC probe requires Apple Silicon macOS." >&2
  exit 1
fi
if [ ! -d "$STAGED_SERVICE" ]; then
  echo "Run scripts/build-computation-runner.sh before the packaged XPC probe." >&2
  exit 1
fi

rm -rf -- "$PROBE"
mkdir -p "$PROBE_CONTENTS/MacOS" "$PROBE_CONTENTS/XPCServices"
SDK_ROOT=$(xcrun --sdk macosx --show-sdk-path)
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  -DSERVICE_NAME="\"$SERVICE_ID\"" \
  "$REPOSITORY_ROOT/src-tauri/computation-xpc/broker.c" \
  -o "$PROBE_CONTENTS/MacOS/zeta-computation-probe"
install -m 0644 "$REPOSITORY_ROOT/src-tauri/computation-xpc/ProbeInfo.plist" \
  "$PROBE_CONTENTS/Info.plist"
cp -R "$STAGED_SERVICE" "$PROBE_CONTENTS/XPCServices/"

SERVICE="$PROBE_CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc"
RUNNER="$SERVICE/Contents/MacOS/zeta-computation-runner"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $SERVICE_ID" \
  "$SERVICE/Contents/Info.plist"
codesign --force --sign - --identifier com.flowzed.zeta.computation-runner \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationRunner.entitlements.plist" \
  "$RUNNER"
codesign --force --sign - --identifier "$SERVICE_ID" \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" \
  "$SERVICE"
codesign --force --sign - \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" \
  "$PROBE"
codesign --verify --deep --strict --verbose=2 "$PROBE"
ZETA_XPC_SMOKE_BROKER="$PROBE_CONTENTS/MacOS/zeta-computation-probe" \
  cargo test --locked --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
  -p computation-runtime --test runner_process \
  packaged_xpc_service_executes_in_its_independent_sandbox -- --ignored --exact
