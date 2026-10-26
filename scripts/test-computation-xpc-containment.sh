#!/bin/sh

set -eu

SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
STAGED_SERVICE="$REPOSITORY_ROOT/src-tauri/computation-service/com.flowzed.zeta.computation-service.xpc"
PROBE="$REPOSITORY_ROOT/target/computation-xpc-containment/ZetaComputationProbe.app"
CONTENTS="$PROBE/Contents"
SERVICE_ID="com.flowzed.zeta.computation-containment-service.run$$"
SECRET="/private/tmp/zeta-computation-xpc-probe-secret"
SYMLINK="/private/tmp/zeta-computation-xpc-probe-link"
LIBRARY="/private/tmp/zeta-computation-xpc-probe.dylib"

cleanup() {
  rm -f -- "$SECRET" "$SYMLINK" "$LIBRARY"
}
trap cleanup EXIT HUP INT TERM

if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ] || [ ! -d "$STAGED_SERVICE" ]; then
  echo "Build the XPC payload on Apple Silicon before the containment probe." >&2
  exit 1
fi

rm -rf -- "$PROBE"
# Qualify a separately signed, known-good transport before the hostile probe.
sh "$SCRIPT_DIRECTORY/test-computation-xpc.sh"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/XPCServices"
SDK_ROOT=$(xcrun --sdk macosx --show-sdk-path)
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  -DSERVICE_NAME="\"$SERVICE_ID\"" \
  "$REPOSITORY_ROOT/src-tauri/computation-xpc/broker.c" \
  -o "$CONTENTS/MacOS/zeta-computation-probe"
install -m 0644 "$REPOSITORY_ROOT/src-tauri/computation-xpc/ProbeInfo.plist" \
  "$CONTENTS/Info.plist"
cp -R "$STAGED_SERVICE" "$CONTENTS/XPCServices/"

SERVICE="$CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc"
RUNNER="$SERVICE/Contents/MacOS/zeta-computation-runner"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $SERVICE_ID" \
  "$SERVICE/Contents/Info.plist"
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  "$REPOSITORY_ROOT/src-tauri/computation-xpc/containment_probe.c" -o "$RUNNER"

cleanup
printf 'sandbox secret\n' > "$SECRET"
ln -s "$SECRET" "$SYMLINK"
printf 'int zeta_probe(void) { return 42; }\n' | \
  env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang -x c -dynamiclib -arch arm64 \
  -isysroot "$SDK_ROOT" -o "$LIBRARY" -

codesign --force --sign - --identifier com.flowzed.zeta.computation-runner \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationRunner.entitlements.plist" "$RUNNER"
codesign --force --sign - --identifier "$SERVICE_ID" \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" "$SERVICE"
codesign --force --sign - \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" "$PROBE"
codesign --verify --deep --strict --verbose=2 "$PROBE"

ZETA_XPC_CONTAINMENT_BROKER="$CONTENTS/MacOS/zeta-computation-probe" \
ZETA_XPC_SMOKE_BROKER="$REPOSITORY_ROOT/target/computation-xpc-probe/ZetaComputationProbe.app/Contents/MacOS/zeta-computation-probe" \
  cargo test --locked --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
  -p computation-runtime --test runner_process \
  packaged_xpc_native_probe_has_no_ambient_authority -- --ignored --exact
