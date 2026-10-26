#!/bin/sh

set -eu

SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)
ICON_DIRECTORY="$REPOSITORY_ROOT/src-tauri/icons"
SOURCE_ICON="$ICON_DIRECTORY/icon-source.png"
MARK_SOURCE="$REPOSITORY_ROOT/public/brand/minutus-mark.svg"
TEMPORARY_DIRECTORY=$(mktemp -d)

cleanup() {
  rm -rf "$TEMPORARY_DIRECTORY"
}
trap cleanup EXIT INT TERM

if ! command -v magick >/dev/null 2>&1; then
  echo "ImageMagick is required to generate the Minutus application icons." >&2
  exit 1
fi

if [ ! -f "$MARK_SOURCE" ]; then
  echo "Missing Minutus vector mark at $MARK_SOURCE." >&2
  exit 1
fi

magick -size 1024x1024 xc:none \
  -colorspace sRGB \
  -type TrueColorAlpha \
  -fill 'rgba(0, 0, 0, 0.2)' \
  -draw 'roundrectangle 112,124 912,924 180,180' \
  -blur 0x24 \
  "$TEMPORARY_DIRECTORY/shadow.png"

magick -size 1024x1024 xc:none \
  -colorspace sRGB \
  -type TrueColorAlpha \
  -fill '#FAF7FB' \
  -stroke '#DDD0DF' \
  -strokewidth 3 \
  -draw 'roundrectangle 100,100 924,924 184,184' \
  "$TEMPORARY_DIRECTORY/tile.png"

# Tauri's SVG renderer preserves the mark's gradient. ImageMagick's built-in
# SVG renderer can silently omit gradient-filled paths on macOS.
pnpm tauri icon "$MARK_SOURCE" --png 600 --output "$TEMPORARY_DIRECTORY/glyph"

magick -size 1024x1024 xc:none \
  -colorspace sRGB \
  -type TrueColorAlpha \
  "$TEMPORARY_DIRECTORY/shadow.png" -composite \
  "$TEMPORARY_DIRECTORY/tile.png" -composite \
  "$TEMPORARY_DIRECTORY/glyph/600x600.png" -gravity center -geometry +0+0 -composite \
  "$SOURCE_ICON"

pnpm tauri icon "$SOURCE_ICON" --output "$ICON_DIRECTORY"

echo "Generated Minutus application icons from $SOURCE_ICON"
