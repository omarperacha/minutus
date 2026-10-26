#!/usr/bin/env python3
"""Repair signatures on Tauri's copied macOS Ollama resources before dev launch."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


MACH_O_MAGICS = {
    b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",
    b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xce",
    b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
}


def sign_runtime(runtime: Path) -> int:
    """Ad-hoc sign invalid Mach-O copies without changing pinned source files."""
    if sys.platform != "darwin":
        return 0
    if not runtime.is_dir() or not (runtime / "payload.sha256").is_file():
        raise ValueError(f"Tauri's copied Ollama runtime is missing: {runtime}")

    files = sorted(runtime.rglob("*"))
    repaired = 0
    for path in files:
        if path.is_symlink():
            raise ValueError(f"Tauri's copied Ollama runtime contains a symlink: {path}")
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            if stream.read(4) not in MACH_O_MAGICS:
                continue
        verified = subprocess.run(
            ["codesign", "--verify", "--strict", str(path)],
            capture_output=True, check=False,
        )
        details = subprocess.run(
            ["codesign", "--display", "--verbose=4", str(path)],
            capture_output=True, check=False,
        )
        if (verified.returncode == 0 and details.returncode == 0
                and b"linker-signed" not in details.stderr):
            continue
        # Replacing a signed copy atomically leaves a prior dev process's mapped
        # executable inode intact while giving the new process valid code pages.
        with tempfile.TemporaryDirectory(prefix=".ollama-sign-", dir=path.parent) as staging:
            signed = Path(staging) / path.name
            shutil.copy2(path, signed)
            subprocess.run(
                ["codesign", "--force", "--sign", "-", str(signed)],
                check=True, capture_output=True,
            )
            subprocess.run(
                ["codesign", "--verify", "--strict", str(signed)],
                check=True, capture_output=True,
            )
            os.replace(signed, path)
        repaired += 1
    return repaired


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: sign_dev_ollama_runtime.py /path/to/ollama-runtime", file=sys.stderr)
        return 2
    try:
        repaired = sign_runtime(Path(sys.argv[1]))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Ollama development signing failed: {error}", file=sys.stderr)
        return 1
    print(f"Verified Tauri's copied Ollama runtime; repaired {repaired} Mach-O signatures.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
