"""Build Zeta's helper from the digest-pinned whisper.cpp source archive."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def build(args):
    pins = json.loads(args.pins.read_text())
    expected = pins["runtime"]["source_archive_sha256"]
    if not args.archive.is_file() or sha256(args.archive) != expected:
        raise ValueError("pinned whisper.cpp source archive is absent or changed")
    if args.output.exists():
        shutil.rmtree(args.output)
    source_staging = args.output / "source-staging"
    source_staging.mkdir(parents=True)
    with tarfile.open(args.archive) as bundle:
        bundle.extractall(source_staging, filter="data")
    roots = list(source_staging.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise ValueError("unexpected whisper.cpp source archive layout")
    source = args.output / "source"
    roots[0].rename(source)
    source_staging.rmdir()
    build_dir = args.output / "build"
    configure = [
        "cmake", "-S", str(args.project.resolve()), "-B", str(build_dir.resolve()),
        "-DWHISPER_SOURCE=" + str(source.resolve()), "-DCMAKE_BUILD_TYPE=Release",
        "-DGGML_METAL="
        + ("ON" if platform.system() == "Darwin" and not args.cpu else "OFF"),
    ]
    if args.macos_deployment_target:
        configure.append("-DCMAKE_OSX_DEPLOYMENT_TARGET=" + args.macos_deployment_target)
    configure_environment = os.environ.copy()
    configure_environment["GIT_CEILING_DIRECTORIES"] = str(args.output.resolve())
    subprocess.run(configure, check=True, env=configure_environment)
    subprocess.run([
        "cmake", "--build", str(build_dir.resolve()), "--config", "Release",
        "--target", "zeta-whisper-helper", "-j", str(args.jobs),
    ], check=True)
    candidates = [build_dir / "zeta-whisper-helper",
                  build_dir / "Release" / "zeta-whisper-helper.exe"]
    helper = next((candidate for candidate in candidates if candidate.is_file()), None)
    if helper is None:
        raise FileNotFoundError("built helper was not found")
    destination = args.output / ("zeta-whisper-helper.exe" if helper.suffix == ".exe" else
                                 "zeta-whisper-helper")
    shutil.copy2(helper, destination)
    print(destination.resolve())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pins", type=Path,
                        default=Path("scripts/asr/models.lock.json"))
    parser.add_argument("--archive", type=Path,
                        default=Path(".cache/asr-001/downloads/whisper-source.tar.gz"))
    parser.add_argument("--project", type=Path, default=Path("scripts/asr"))
    parser.add_argument("--output", type=Path, default=Path(".cache/asr-003"))
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--cpu", action="store_true",
                        help="disable optional acceleration for the portable CPU build")
    parser.add_argument("--macos-deployment-target",
                        help="set the explicit deployment target for a macOS package build")
    arguments = parser.parse_args()
    if not 1 <= arguments.jobs <= 64:
        parser.error("--jobs must be between 1 and 64")
    build(arguments)
