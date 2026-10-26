#!/bin/sh

set -eu

if [ "$#" -ne 1 ]; then
  echo "Usage: scripts/verify-macos-bundle.sh /absolute/path/to/Minutus.app" >&2
  exit 2
fi

APPLICATION="$1"
CONTENTS="$APPLICATION/Contents"
OLLAMA_RUNTIME="$CONTENTS/Resources/ollama-runtime"
SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
BROKER="$CONTENTS/MacOS/zeta-computation-broker"
WHISPER_HELPER="$CONTENTS/MacOS/zeta-whisper-helper"
XPC_SERVICE="$CONTENTS/XPCServices/com.flowzed.zeta.computation-service.xpc"
SERVICE_EXECUTABLE="$XPC_SERVICE/Contents/MacOS/zeta-computation-service"
RUNNER="$XPC_SERVICE/Contents/MacOS/zeta-computation-runner"
COMPUTATION_DIGEST_MANIFEST="$CONTENTS/Resources/computation-service.sha256"
SPEECH_RUNTIME="$CONTENTS/Resources/speech-runtime"
SPEECH_DIGEST_MANIFEST="$SPEECH_RUNTIME/helper.signed.sha256"

case "$APPLICATION" in
  /*/Minutus.app) EXPECTED_BUNDLE_ID=com.omarperacha.zeta ;;
  /*/Minutus\ Staging.app) EXPECTED_BUNDLE_ID=com.omarperacha.zeta.staging ;;
  *)
    echo "Expected an absolute path ending in Minutus.app or Minutus Staging.app." >&2
    exit 2
    ;;
esac

for required in \
  "$CONTENTS/MacOS/zeta-desktop" \
  "$CONTENTS/MacOS/zeta-tokenizer" \
  "$WHISPER_HELPER" \
  "$BROKER" \
  "$SERVICE_EXECUTABLE" \
  "$RUNNER" \
  "$COMPUTATION_DIGEST_MANIFEST" \
  "$SPEECH_DIGEST_MANIFEST" \
  "$SPEECH_RUNTIME/runtime.lock" \
  "$SPEECH_RUNTIME/WHISPER-CPP-LICENSE.txt" \
  "$SPEECH_RUNTIME/OPENAI-WHISPER-LICENSE.txt" \
  "$OLLAMA_RUNTIME/ollama" \
  "$OLLAMA_RUNTIME/payload.sha256" \
  "$OLLAMA_RUNTIME/OLLAMA-LICENSE"; do
  if [ ! -f "$required" ]; then
    echo "The app bundle is missing a required signed runtime file." >&2
    exit 1
  fi
done

read -r SPEECH_DIGEST SPEECH_PATH < "$SPEECH_DIGEST_MANIFEST"
if [ "$SPEECH_PATH" != "MacOS/zeta-whisper-helper" ] || \
  [ ${#SPEECH_DIGEST} -ne 64 ] || \
  [ "$SPEECH_DIGEST" != "$(shasum -a 256 "$WHISPER_HELPER" | awk '{print $1}')" ]; then
  echo "The signed Whisper helper differs from its sealed digest." >&2
  exit 1
fi

if [ -e "$CONTENTS/MacOS/zeta-computation-runner" ]; then
  echo "The unsafe raw computation child is still present in the app executable directory." >&2
  exit 1
fi
if [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$XPC_SERVICE/Contents/Info.plist")" != \
  "com.flowzed.zeta.computation-service" ] || \
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$XPC_SERVICE/Contents/Info.plist")" != \
  "zeta-computation-service" ]; then
  echo "The computation XPC bundle metadata differs from reviewed policy." >&2
  exit 1
fi
EXPECTED_COMPUTATION_FILES=4
if [ "$(wc -l < "$COMPUTATION_DIGEST_MANIFEST" | tr -d ' ')" != "$EXPECTED_COMPUTATION_FILES" ]; then
  echo "The signed computation payload manifest has the wrong file set." >&2
  exit 1
fi
while IFS='  ' read -r expected_digest relative_path; do
  relative_path=${relative_path#./}
  case "$relative_path" in
    MacOS/zeta-computation-broker|\
    XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/Info.plist|\
    XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-runner|\
    XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-service) ;;
    *)
      echo "The signed computation payload manifest contains an unexpected path." >&2
      exit 1
      ;;
  esac
  actual_digest=$(shasum -a 256 "$CONTENTS/$relative_path" | awk '{print $1}')
  if [ ${#expected_digest} -ne 64 ] || [ "$expected_digest" != "$actual_digest" ]; then
    echo "A signed computation payload differs from its sealed digest." >&2
    exit 1
  fi
done < "$COMPUTATION_DIGEST_MANIFEST"

verify_computation_executable() {
  EXECUTABLE="$1"
  EXPECTED_IDENTIFIER="$2"
  EXPECTED_ENTITLEMENT_COUNT="$3"
  if [ "$(lipo -archs "$EXECUTABLE")" != "arm64" ]; then
    echo "A computation executable is not arm64-only." >&2
    exit 1
  fi
  if ! codesign --display --verbose=4 "$EXECUTABLE" 2>&1 | \
    grep -q "^Identifier=$EXPECTED_IDENTIFIER$"; then
    echo "A computation executable has the wrong signed identifier." >&2
    exit 1
  fi
  if ! otool -L "$EXECUTABLE" | tail -n +2 | awk '{print $1}' | awk '
    !/^\/usr\/lib\// && !/^\/System\/Library\// { exit 1 }
  '; then
    echo "A computation executable has a non-system dynamic dependency." >&2
    exit 1
  fi
  EXECUTABLE_ENTITLEMENTS=$(codesign --display --entitlements :- "$EXECUTABLE" 2>/dev/null)
  if ! printf '%s' "$EXECUTABLE_ENTITLEMENTS" | grep -q 'com.apple.security.app-sandbox'; then
    echo "A computation executable is not App Sandbox confined." >&2
    exit 1
  fi
  ENTITLEMENT_COUNT=$(printf '%s' "$EXECUTABLE_ENTITLEMENTS" | \
    awk -F'<key>' '{ count += NF - 1 } END { print count }')
  if [ "$ENTITLEMENT_COUNT" -ne "$EXPECTED_ENTITLEMENT_COUNT" ]; then
    echo "A computation executable entitlement set differs from reviewed policy." >&2
    exit 1
  fi
}

verify_computation_executable "$BROKER" com.flowzed.zeta.computation-broker 2
verify_computation_executable "$SERVICE_EXECUTABLE" com.flowzed.zeta.computation-service 1
verify_computation_executable "$RUNNER" com.flowzed.zeta.computation-runner 2

if [ "$(lipo -archs "$WHISPER_HELPER")" != "arm64" ]; then
  echo "The Whisper helper is not arm64-only." >&2
  exit 1
fi
if ! codesign --display --verbose=4 "$WHISPER_HELPER" 2>&1 | \
  grep -q '^Identifier=com.flowzed.zeta.whisper-helper$'; then
  echo "The Whisper helper has the wrong signed identifier." >&2
  exit 1
fi
if ! otool -L "$WHISPER_HELPER" | tail -n +2 | awk '{print $1}' | awk '
  !/^\/usr\/lib\// && !/^\/System\/Library\// { exit 1 }
'; then
  echo "The Whisper helper has a non-system dynamic dependency." >&2
  exit 1
fi
WHISPER_ENTITLEMENTS=$(codesign --display --entitlements :- "$WHISPER_HELPER" 2>/dev/null)
for entitlement in com.apple.security.app-sandbox com.apple.security.inherit; do
  if ! printf '%s' "$WHISPER_ENTITLEMENTS" | grep -q "$entitlement"; then
    echo "The Whisper helper cannot inherit the application sandbox." >&2
    exit 1
  fi
done
WHISPER_ENTITLEMENT_COUNT=$(printf '%s' "$WHISPER_ENTITLEMENTS" | \
  awk -F'<key>' '{ count += NF - 1 } END { print count }')
if [ "$WHISPER_ENTITLEMENT_COUNT" -ne 2 ]; then
  echo "The Whisper helper entitlement set differs from reviewed policy." >&2
  exit 1
fi

for inherited in "$BROKER" "$RUNNER"; do
  if ! codesign --display --entitlements :- "$inherited" 2>&1 | \
    grep -q 'com.apple.security.inherit'; then
    echo "A computation child cannot inherit its immediate parent sandbox." >&2
    exit 1
  fi
done
SERVICE_ENTITLEMENTS=$(codesign --display --entitlements :- "$SERVICE_EXECUTABLE" 2>/dev/null)
for forbidden in \
  com.apple.security.inherit \
  com.apple.security.network.client \
  com.apple.security.network.server \
  com.apple.security.files.user-selected \
  com.apple.security.files.bookmarks \
  keychain-access-groups \
  com.apple.security.application-groups; do
  if printf '%s' "$SERVICE_ENTITLEMENTS" | grep -q "$forbidden"; then
    echo "The independent computation service has a forbidden entitlement." >&2
    exit 1
  fi
done

if ! cmp -s "$REPOSITORY_ROOT/src-tauri/ollama-runtime/payload.sha256" \
  "$OLLAMA_RUNTIME/payload.sha256"; then
  echo "The app does not contain the pinned Ollama payload manifest." >&2
  exit 1
fi

EXPECTED_FILES=$(wc -l < "$OLLAMA_RUNTIME/payload.sha256" | tr -d ' ')
ACTUAL_FILES=$(find "$OLLAMA_RUNTIME" -type f ! -name payload.sha256 | wc -l | tr -d ' ')
if [ "$EXPECTED_FILES" != "$ACTUAL_FILES" ]; then
  echo "The signed Ollama runtime file set differs from its pinned manifest." >&2
  exit 1
fi
while IFS='  ' read -r _digest relative_path; do
  relative_path=${relative_path#./}
  if [ ! -f "$OLLAMA_RUNTIME/$relative_path" ]; then
    echo "The signed Ollama runtime is missing a pinned payload entry." >&2
    exit 1
  fi
done < "$OLLAMA_RUNTIME/payload.sha256"

find "$CONTENTS" -type f -print | while IFS= read -r bundled_file; do
  if file "$bundled_file" | grep -q 'Mach-O'; then
    codesign --verify --strict --verbose=2 "$bundled_file"
  fi
done

codesign --verify --deep --strict --verbose=2 "$APPLICATION"
if ! codesign --display --verbose=4 "$APPLICATION" 2>&1 | \
  grep -q "^Identifier=$EXPECTED_BUNDLE_ID$" || \
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CONTENTS/Info.plist")" != "$EXPECTED_BUNDLE_ID" ]; then
  echo "The signed application has the wrong bundle signing identifier." >&2
  exit 1
fi
ENTITLEMENTS=$(codesign --display --entitlements :- "$APPLICATION" 2>/dev/null)
for entitlement in \
  com.apple.security.app-sandbox \
  com.apple.security.network.client \
  com.apple.security.network.server; do
  if ! printf '%s' "$ENTITLEMENTS" | grep -q "$entitlement"; then
    echo "The signed app is missing a required runtime entitlement." >&2
    exit 1
  fi
done
if printf '%s' "$ENTITLEMENTS" | grep -q 'keychain-access-groups'; then
  echo "The signed application must use its private default Keychain access group." >&2
  exit 1
fi
if [ "$(/usr/libexec/PlistBuddy -c 'Print :NSMicrophoneUsageDescription' "$CONTENTS/Info.plist")" != \
  "Minutus uses your microphone only when you start local speech dictation." ] || \
  [ "$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$CONTENTS/Info.plist")" != "13.3" ]; then
  echo "The application microphone purpose or minimum macOS version is incorrect." >&2
  exit 1
fi

if [ "${ZETA_RELEASE_VERIFICATION:-0}" = "1" ]; then
  if [ "$EXPECTED_BUNDLE_ID" != "com.omarperacha.zeta" ]; then
    echo "Release verification refuses the staging identity." >&2
    exit 1
  fi
  # The codesign identifier alone does not grant Data Protection Keychain access.
  python3 "$SCRIPT_DIRECTORY/macos-wallet-signing.py" verify "$APPLICATION"
  for release_code in "$APPLICATION" "$WHISPER_HELPER" "$BROKER" "$XPC_SERVICE" "$RUNNER"; do
    if ! codesign --display --verbose=4 "$release_code" 2>&1 | \
      grep -q '^Authority=Developer ID Application:'; then
      echo "Release verification requires Developer ID signatures on every computation boundary." >&2
      exit 1
    fi
  done
  spctl --assess --type execute --verbose=2 "$APPLICATION"
  xcrun stapler validate "$APPLICATION"
fi

echo "Verified signed Minutus app, Whisper helper, isolated computation XPC service and bundled inference runtimes."
