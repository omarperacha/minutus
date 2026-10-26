#!/bin/sh
# Exercise the real Rust supervisor from App Sandbox, not an unsandboxed cargo host.
set -eu
SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
PROBE_ROOT="$REPOSITORY_ROOT/target/computation-sandbox-host"
PROBE="$PROBE_ROOT/ComputationSandboxHost.app"
CONTENTS="$PROBE/Contents"
SERVICE_ID="com.flowzed.zeta.computation-sandbox-host.run$$"
SERVICE_SOURCE="$REPOSITORY_ROOT/src-tauri/computation-service/com.flowzed.zeta.computation-service.xpc"
MODE=${1:-normal}
case "$MODE" in normal|stress|cpu|memory|wall|early_exit|closed_input) ;; *) exit 2 ;; esac
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/XPCServices"
cargo test --locked --no-run --message-format=json --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
  -p computation-runtime --test runner_process > "$PROBE_ROOT/artifacts.jsonl"
TEST_EXECUTABLE=$(python3 -c 'import json,sys; print(next(x["executable"] for x in map(json.loads, open(sys.argv[1])) if x.get("executable") and x.get("target",{}).get("name")=="runner_process"))' "$PROBE_ROOT/artifacts.jsonl")
install -m 0755 "$TEST_EXECUTABLE" "$CONTENTS/MacOS/computation-tests"
install -m 0644 "$REPOSITORY_ROOT/src-tauri/computation-xpc/ProbeInfo.plist" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleExecutable computation-tests' "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $SERVICE_ID.host" "$CONTENTS/Info.plist"
# Remove only this generated fixture's old service before replacing its payload.
if [ -d "$CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc" ]; then
  rm -rf -- "$CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc"
fi
cp -R "$SERVICE_SOURCE" "$CONTENTS/XPCServices/"
SERVICE="$CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc"
RUNNER="$SERVICE/Contents/MacOS/zeta-computation-runner"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $SERVICE_ID" "$SERVICE/Contents/Info.plist"
SDK_ROOT=$(xcrun --sdk macosx --show-sdk-path)
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  "$REPOSITORY_ROOT/src-tauri/computation-xpc/worker_tests.c" -o "$PROBE_ROOT/worker-tests"
"$PROBE_ROOT/worker-tests"
env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
  -std=c17 -Wall -Wextra -Wconversion -Werror -fblocks \
  -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
  -DSERVICE_NAME="\"$SERVICE_ID\"" "$REPOSITORY_ROOT/src-tauri/computation-xpc/broker.c" \
  -o "$CONTENTS/MacOS/zeta-computation-probe"
if [ "$MODE" != normal ] && [ "$MODE" != stress ]; then
  env -u CPATH -u CPLUS_INCLUDE_PATH xcrun clang \
    -std=c17 -Wall -Wextra -Wconversion -Werror \
    -isysroot "$SDK_ROOT" -mmacosx-version-min=13.3 -arch arm64 \
    -DPROBE_MODE="\"$MODE\"" "$REPOSITORY_ROOT/src-tauri/computation-xpc/resource_probe.c" -o "$RUNNER"
fi
codesign --force --sign - --identifier com.flowzed.zeta.computation-runner \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationRunner.entitlements.plist" "$RUNNER"
codesign --force --sign - --identifier "$SERVICE_ID" \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" "$SERVICE"
codesign --force --sign - --identifier "$SERVICE_ID.broker" \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationBroker.entitlements.plist" "$CONTENTS/MacOS/zeta-computation-probe"
codesign --force --sign - \
  --entitlements "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" "$PROBE"
codesign --verify --deep --strict --verbose=2 "$PROBE"
if [ "$MODE" = normal ]; then
  TEST=packaged_xpc_service_executes_in_its_independent_sandbox
elif [ "$MODE" = stress ]; then
  TEST=packaged_xpc_child_accounting_survives_repeated_fast_exits
else
  TEST=packaged_xpc_native_worker_limits_are_independent_of_the_interpreter
fi
ZETA_XPC_RESOURCE_MODE="$MODE" \
ZETA_XPC_SMOKE_BROKER="$CONTENTS/MacOS/zeta-computation-probe" \
  "$CONTENTS/MacOS/computation-tests" "$TEST" --exact --ignored --nocapture
