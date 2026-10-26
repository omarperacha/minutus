#!/bin/sh

set -eu

if [ "$#" -ne 2 ]; then
  echo "Usage: scripts/sign-macos-bundle.sh /absolute/path/to/Minutus.app 'Developer ID Application: ...'" >&2
  echo "Use - for a local ad-hoc source build; wallet functionality needs no Apple developer credentials." >&2
  exit 2
fi

APPLICATION="$1"
IDENTITY="$2"
SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
APP_ENTITLEMENTS="${ZETA_APP_ENTITLEMENTS:-$REPOSITORY_ROOT/src-tauri/Entitlements.plist}"
CONTENTS="$APPLICATION/Contents"
STAGED_SERVICE="$CONTENTS/Resources/computation-service/com.flowzed.zeta.computation-service.xpc"
XPC_DIRECTORY="$CONTENTS/XPCServices"
XPC_SERVICE="$XPC_DIRECTORY/com.flowzed.zeta.computation-service.xpc"
BROKER="$CONTENTS/MacOS/zeta-computation-broker"
WHISPER_HELPER="$CONTENTS/MacOS/zeta-whisper-helper"
RAW_RUNNER="$CONTENTS/MacOS/zeta-computation-runner"
SERVICE_RUNNER="$XPC_SERVICE/Contents/MacOS/zeta-computation-runner"
SIGNED_MANIFEST="$CONTENTS/Resources/computation-service.sha256"
SPEECH_SIGNED_MANIFEST="$CONTENTS/Resources/speech-runtime/helper.signed.sha256"

case "$APPLICATION" in
  /*/Minutus.app|/*/Minutus\ Staging.app) ;;
  *)
    echo "Expected an absolute path ending in Minutus.app or Minutus Staging.app." >&2
    exit 2
    ;;
esac

sign_file() {
  if [ "$IDENTITY" = "-" ]; then
    codesign --force --sign - "$1"
  else
    codesign --force --timestamp --options runtime --sign "$IDENTITY" "$1"
  fi
}

sign_with_policy() {
  IDENTIFIER="$1"
  ENTITLEMENTS="$2"
  TARGET="$3"
  if [ "$IDENTITY" = "-" ]; then
    codesign --force --sign - \
      --identifier "$IDENTIFIER" --entitlements "$ENTITLEMENTS" "$TARGET"
  else
    codesign --force --timestamp --options runtime --sign "$IDENTITY" \
      --identifier "$IDENTIFIER" --entitlements "$ENTITLEMENTS" "$TARGET"
  fi
}

if [ ! -d "$STAGED_SERVICE" ] || [ ! -f "$RAW_RUNNER" ] || [ ! -f "$BROKER" ] || \
  [ ! -f "$WHISPER_HELPER" ] || [ ! -f "$CONTENTS/Resources/speech-runtime/runtime.lock" ]; then
  echo "The app is missing its reviewed computation XPC payload." >&2
  exit 1
fi
(
  cd "$CONTENTS/Resources/computation-service"
  shasum -a 256 -c payload.sha256
)
mkdir -p "$XPC_DIRECTORY"
mv "$STAGED_SERVICE" "$XPC_SERVICE"
rm "$CONTENTS/Resources/computation-service/payload.sha256"
rmdir "$CONTENTS/Resources/computation-service"
if ! cmp -s "$RAW_RUNNER" "$SERVICE_RUNNER"; then
  echo "The direct and XPC-staged computation runners differ." >&2
  exit 1
fi
rm "$RAW_RUNNER"

find "$CONTENTS/Resources/ollama-runtime" -type f -print | while IFS= read -r runtime_file; do
  if file "$runtime_file" | grep -q 'Mach-O'; then
    sign_file "$runtime_file"
  fi
done

sign_file "$CONTENTS/MacOS/zeta-tokenizer"
sign_with_policy com.flowzed.zeta.whisper-helper \
  "$REPOSITORY_ROOT/src-tauri/WhisperHelper.entitlements.plist" "$WHISPER_HELPER"
sign_with_policy com.flowzed.zeta.computation-runner \
  "$REPOSITORY_ROOT/src-tauri/ComputationRunner.entitlements.plist" "$SERVICE_RUNNER"
sign_with_policy com.flowzed.zeta.computation-service \
  "$REPOSITORY_ROOT/src-tauri/ComputationService.entitlements.plist" "$XPC_SERVICE"
sign_with_policy com.flowzed.zeta.computation-broker \
  "$REPOSITORY_ROOT/src-tauri/ComputationBroker.entitlements.plist" "$BROKER"

: > "$SIGNED_MANIFEST"
for signed_path in \
  MacOS/zeta-computation-broker \
  XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/Info.plist \
  XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-runner \
  XPCServices/com.flowzed.zeta.computation-service.xpc/Contents/MacOS/zeta-computation-service; do
  SIGNED_SHA256=$(shasum -a 256 "$CONTENTS/$signed_path" | awk '{print $1}')
  printf '%s  %s\n' "$SIGNED_SHA256" "$signed_path" >> "$SIGNED_MANIFEST"
done
chmod 0644 "$SIGNED_MANIFEST"
WHISPER_SHA256=$(shasum -a 256 "$WHISPER_HELPER" | awk '{print $1}')
printf '%s  %s\n' "$WHISPER_SHA256" "MacOS/zeta-whisper-helper" > "$SPEECH_SIGNED_MANIFEST"
chmod 0644 "$SPEECH_SIGNED_MANIFEST"
sign_file "$CONTENTS/MacOS/zeta-desktop"

if [ "$IDENTITY" = "-" ]; then
  codesign --force --sign - \
    --entitlements "$APP_ENTITLEMENTS" "$APPLICATION"
else
  codesign --force --timestamp --options runtime --sign "$IDENTITY" \
    --entitlements "$APP_ENTITLEMENTS" "$APPLICATION"
fi

echo "Signed Minutus app, Whisper helper, isolated computation XPC service and every nested runtime Mach-O file."
