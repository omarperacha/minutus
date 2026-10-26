#!/usr/bin/env python3
"""Build, sign and verify a non-launching macOS bundle for trusted feature tests."""

import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

from x402_debug_env import environment
from build_cache import CacheBusyError, maintain_workspace as maintain

REPOSITORY = Path(__file__).resolve().parent.parent
CONFIGURATION = REPOSITORY / ".zeta-stage.json"
APPLICATION = REPOSITORY / "target/debug/bundle/macos/Minutus Staging.app"
LEGACY_APPLICATION = REPOSITORY / "target/debug/bundle/macos/Minutus.app"
HOT_RELOAD_EXECUTABLE = REPOSITORY / "target/debug/zeta-desktop"
IDENTITY_PATTERN = re.compile(r"[0-9A-Fa-f]{40}")


def load_configuration(path=CONFIGURATION):
    """Load only the two reviewed machine-local staging settings."""
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise ValueError(
            "Create .zeta-stage.json from .zeta-stage.example.json before staging."
        ) from error
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("The staging configuration is not valid UTF-8 JSON.") from error
    if not isinstance(raw, dict) or set(raw) != {"identity", "profile"}:
        raise ValueError("The staging configuration must contain only identity and profile.")
    identity = raw["identity"]
    profile = raw["profile"]
    if not isinstance(identity, str) or IDENTITY_PATTERN.fullmatch(identity) is None:
        raise ValueError("The staging identity must be one exact 40-character certificate hash.")
    if not isinstance(profile, str) or not profile or "\0" in profile:
        raise ValueError("The staging profile path is invalid.")
    profile_path = Path(profile)
    if not profile_path.is_absolute():
        raise ValueError("The staging profile path must be absolute.")
    profile_path = profile_path.resolve()
    if not profile_path.is_file():
        raise ValueError("The configured staging provisioning profile does not exist.")
    return identity.upper(), profile_path


def process_is_running(executable):
    """Return whether one exact executable is currently running."""
    if not executable.exists():
        return False
    result = subprocess.run(
        ["pgrep", "-f", f"^{re.escape(str(executable))}$"],
        check=False,
        capture_output=True,
    )
    return result.returncode == 0


def bundle_process_is_running(application=APPLICATION):
    """Return whether any main or helper executable is running from the bundle."""
    if not application.exists():
        return False
    result = subprocess.run(
        ["pgrep", "-f", f"^{re.escape(str(application))}/Contents/"],
        check=False,
        capture_output=True,
    )
    return result.returncode == 0


def stage(identity="-", profile=None, application=APPLICATION):
    """Create a fresh bundle, then delegate signing and strict verification."""
    if (bundle_process_is_running(application)
            or bundle_process_is_running(LEGACY_APPLICATION)
            or process_is_running(HOT_RELOAD_EXECUTABLE)):
        raise ValueError("Quit every running Minutus app and tauri:dev before staging.")
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        raise ValueError("pnpm is unavailable on PATH.")
    try:
        build_and_sign(pnpm, identity, profile, application)
    finally:
        try:
            maintain()
        except CacheBusyError as error:
            print(str(error), file=sys.stderr)


def build_and_sign(pnpm, identity, profile, application):
    """Keep signed app verification complete before trimming compiler artifacts."""
    started = time.monotonic()
    print("Building frontend and native app (Cargo timing report enabled)…", flush=True)
    subprocess.run(
        [pnpm, "tauri", "build", "--debug", "--config", "src-tauri/tauri.stage.conf.json", "--bundles", "app", "--", "--timings"],
        cwd=REPOSITORY,
        env=environment(REPOSITORY),
        check=True,
    )
    built = time.monotonic()
    print(f"Build and bundle: {built - started:.1f}s. Signing and verifying…", flush=True)
    if identity == "-":
        subprocess.run(
            ["sh", str(REPOSITORY / "scripts/sign-macos-bundle.sh"), str(application), "-"],
            cwd=REPOSITORY, check=True,
        )
        subprocess.run(
            ["sh", str(REPOSITORY / "scripts/verify-macos-bundle.sh"), str(application)],
            cwd=REPOSITORY, check=True,
        )
    else:
        subprocess.run(
            [sys.executable, str(REPOSITORY / "scripts/macos-wallet-signing.py"),
             "sign", str(application), identity, str(profile)],
            cwd=REPOSITORY, check=True,
        )
    finished = time.monotonic()
    print(
        f"Signing and verification: {finished - built:.1f}s. "
        f"Total staging: {finished - started:.1f}s.",
        flush=True,
    )


def main():
    if sys.platform != "darwin":
        print("Staging a signed Minutus app requires macOS.", file=sys.stderr)
        return 1
    try:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("--apple-signing", action="store_true", help="Explicitly use .zeta-stage.json for Apple signing; local wallet use needs no certificate or profile")
        args = parser.parse_args()
        if args.apple_signing:
            stage(*load_configuration())
        else:
            stage()
    except (ValueError, OSError, RuntimeError) as error:
        print(f"Minutus staging stopped: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError:
        print(
            "Minutus staging stopped: the build, signing or verification command failed.",
            file=sys.stderr,
        )
        return 1
    print(f"Signed staging app ready: {APPLICATION}")
    print(f'Launch explicitly with: open "{APPLICATION}"')
    return 0


if __name__ == "__main__":
    sys.exit(main())
