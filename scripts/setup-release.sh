#!/bin/bash
# Prepare a local source release without launching it or changing app data.
set -eo pipefail

if [[ "${1:-}" == "--help" && "$#" == 1 ]]; then
  echo "Usage: ./scripts/setup-release.sh"
  echo "Install locked dependencies, bootstrap native inputs, check and build. Does not launch Minutus."
  exit 0
fi
if [[ "$#" != 0 ]]; then
  echo "Usage: ./scripts/setup-release.sh" >&2
  exit 2
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
fail() { echo "Source build stopped: $*" >&2; exit 1; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || fail "Apple Silicon macOS is required."
for tool in python3 cmake xcrun rustup cargo rustc; do
  command -v "$tool" >/dev/null || fail "Install the prerequisite '$tool'; see docs/build.md."
done
xcrun --find clang >/dev/null || fail "Install the Xcode command-line tools."
python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 12) else "Python 3.12 or later is required.")'
python3 -c 'import re, subprocess, sys; version = re.search(r"cmake version (\d+)\.(\d+)", subprocess.check_output(["cmake", "--version"], text=True)); sys.exit(0 if version and tuple(map(int, version.groups())) >= (3, 24) else "CMake 3.24 or later is required.")'

NODE_VERSION="$(tr -d '[:space:]' < .nvmrc)"
NVM_SCRIPT="${NVM_DIR:-$HOME/.nvm}/nvm.sh"
if [[ -f "$NVM_SCRIPT" ]]; then
  # nvm is a shell function, so load it explicitly in this new shell.
  source "$NVM_SCRIPT" --no-use
  nvm install "$NODE_VERSION"
  nvm use "$NODE_VERSION"
fi
command -v node >/dev/null || fail "Install Node.js $NODE_VERSION or install nvm first."
[[ "$(node --version)" == "v$NODE_VERSION" ]] || fail "Select Node.js $NODE_VERSION; see .nvmrc."
PNPM_VERSION="$(python3 -c 'import json; print(json.load(open("package.json"))["packageManager"].removeprefix("pnpm@"))')"
command -v pnpm >/dev/null || fail "Run: npm install --global pnpm@$PNPM_VERSION"
[[ "$(pnpm --version)" == "$PNPM_VERSION" ]] || fail "Run: npm install --global pnpm@$PNPM_VERSION"

pnpm install --frozen-lockfile
python3 scripts/bootstrap.py
pnpm check
pnpm release:build
echo "Build complete. To launch Minutus, run:"
echo "open target/aarch64-apple-darwin/release/bundle/macos/Minutus.app"
