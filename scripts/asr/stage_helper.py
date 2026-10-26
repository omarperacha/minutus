"""Build and stage the pinned Whisper helper for Tauri packaging."""

import argparse
import hashlib
from pathlib import Path
import platform
import shutil
import subprocess
import sys


HOST_TARGETS = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Windows", "AMD64"): "x86_64-pc-windows-msvc",
}


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def run(arguments: argparse.Namespace) -> None:
    host = (platform.system(), platform.machine())
    detected = HOST_TARGETS.get(host)
    if detected != arguments.target:
        raise ValueError(f"target {arguments.target} does not match host {host[0]}/{host[1]}")
    repository = Path(__file__).resolve().parents[2]
    output = repository / ".cache/asr-008"
    command = [
        sys.executable,
        str(repository / "scripts/asr/build_helper.py"),
        "--output",
        str(output),
        "--jobs",
        str(arguments.jobs),
    ]
    if platform.system() == "Darwin":
        command.extend(["--macos-deployment-target", "13.3"])
    else:
        command.append("--cpu")
    subprocess.run(command, cwd=repository, check=True)

    suffix = ".exe" if platform.system() == "Windows" else ""
    helper = output / f"zeta-whisper-helper{suffix}"
    if not helper.is_file() or helper.is_symlink():
        raise FileNotFoundError("the pinned helper build produced no regular executable")
    if platform.system() == "Darwin":
        architectures = subprocess.check_output(["lipo", "-archs", helper], text=True).strip()
        if architectures != "arm64":
            raise ValueError("the macOS Whisper helper is not arm64-only")
        dependencies = subprocess.check_output(["otool", "-L", helper], text=True)
        for line in dependencies.splitlines()[1:]:
            dependency = line.strip().split(" ", 1)[0]
            if not dependency.startswith(("/usr/lib/", "/System/Library/")):
                raise ValueError(f"non-system helper dependency: {dependency}")

    destination = repository / "src-tauri/binaries" / f"zeta-whisper-helper-{arguments.target}{suffix}"
    shutil.copy2(helper, destination)
    destination.chmod(0o755)
    digest = sha256(destination)
    destination.with_suffix(destination.suffix + ".sha256").write_text(
        f"{digest}  {destination.name}\n"
    )
    print(destination)
    print(f"SHA-256: {digest}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=sorted(HOST_TARGETS.values()))
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 64:
        parser.error("--jobs must be between 1 and 64")
    run(args)
