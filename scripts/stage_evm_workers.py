#!/usr/bin/env python3
"""Stage separate signed EVM helpers before host-build identity embedding."""
import argparse
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import struct
import sys

from evm_notices import verify_notice_sources, verify_bundled_notices

ROOT = Path(__file__).resolve().parent.parent
NAMES = ("minutus-evm-simulator", "minutus-base-offline-simulator")
TARGET = "aarch64-apple-darwin"


def identity(path):
    subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(path)], check=True, capture_output=True)
    result = subprocess.run(["/usr/bin/codesign", "-d", "--verbose=4", str(path)], check=True, capture_output=True, text=True)
    if "Hash choices=sha256" not in result.stderr.splitlines():
        raise ValueError("EVM worker must use only SHA256 code-directory hashes")
    matches = re.findall(r"^CDHash=([0-9a-f]{40})$", result.stderr, re.MULTILINE)
    if len(matches) != 1:
        raise ValueError("EVM worker has no unique code-directory hash")
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return digest, matches[0]


def stage(signing_identity="-"):
    if sys.platform != "darwin":
        raise ValueError("EVM worker staging requires Apple Silicon macOS")
    if signing_identity != "-" and not re.fullmatch(r"[0-9a-fA-F]{40}", signing_identity):
        raise ValueError("Expected one exact signing certificate hash")
    verify_notice_sources(ROOT)
    subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "--target", TARGET, "-p", "evm-simulation", "--bins"], cwd=ROOT, check=True)
    records = ["minutus-evm-workers-v1"]
    for name in NAMES:
        destination = ROOT / "src-tauri/binaries" / f"{name}-{TARGET}"
        if destination.is_symlink():
            raise ValueError("EVM worker destination is indirect")
        shutil.copyfile(ROOT / f"target/{TARGET}/release" / name, destination)
        destination.chmod(0o755)
        if subprocess.check_output(["/usr/bin/lipo", "-archs", str(destination)], text=True).strip() != "arm64":
            raise ValueError("EVM worker must be arm64-only")
        dependencies = subprocess.check_output(["/usr/bin/otool", "-L", str(destination)], text=True).splitlines()[1:]
        if any(not line.strip().startswith(("/usr/lib/", "/System/Library/")) for line in dependencies):
            raise ValueError("EVM worker has a non-system dynamic dependency")
        command = ["/usr/bin/codesign", "--force", "--sign", signing_identity, "--identifier", f"ai.minutus.{name}", "--digest-algorithm=sha256"]
        if signing_identity != "-":
            command += ["--timestamp", "--options", "runtime"]
        subprocess.run([*command, str(destination)], check=True)
        digest, cdhash = identity(destination)
        records.append(f"{name} {digest} {cdhash}")
    directory = ROOT / "src-tauri/evm-workers"
    directory.mkdir(exist_ok=True)
    temporary = directory / "manifest.txt.incomplete"
    temporary.write_text("\n".join(records) + "\n", encoding="ascii")
    temporary.replace(directory / "manifest.txt")


def embedded_manifest(executable):
    """Read the host's exact bounded arm64 Mach-O manifest, not a sidecar pin."""
    with executable.open("rb") as stream:
        header = stream.read(32)
        if len(header) != 32:
            raise ValueError("Truncated host Mach-O")
        magic, cpu, _, filetype, count, size, _, _ = struct.unpack("<IiiIIIII", header)
        if magic != 0xFEEDFACF or cpu != 0x0100000C or filetype != 2 or count > 4096 or size > 4 * 1024 * 1024:
            raise ValueError("Unsupported host Mach-O")
        commands = stream.read(size)
        offset, found = 0, []
        for _ in range(count):
            if offset + 8 > len(commands):
                raise ValueError("Truncated Mach-O command")
            command, length = struct.unpack_from("<II", commands, offset)
            if length < 8 or offset + length > len(commands):
                raise ValueError("Invalid Mach-O command")
            if command == 0x19:
                if length < 72:
                    raise ValueError("Truncated Mach-O segment")
                sections = struct.unpack_from("<I", commands, offset + 64)[0]
                if 72 + sections * 80 > length:
                    raise ValueError("Truncated Mach-O sections")
                for index in range(sections):
                    section = offset + 72 + index * 80
                    name, segment = struct.unpack_from("<16s16s", commands, section)
                    if name.rstrip(b"\0") == b"__evm_workers" and segment.rstrip(b"\0") == b"__TEXT":
                        amount, location = struct.unpack_from("<QI", commands, section + 40)
                        if not 0 < amount <= 1024:
                            raise ValueError("Invalid host worker manifest size")
                        stream.seek(location)
                        value = stream.read(amount)
                        if len(value) != amount:
                            raise ValueError("Truncated host worker manifest")
                        found.append(value)
            offset += length
        if len(found) != 1 or offset != size:
            raise ValueError("Missing or ambiguous host worker manifest")
    return found[0].decode("ascii").splitlines()


def verify_bundle(application):
    """Bundling/signing must preserve helpers pinned into this exact host image."""
    verify_bundled_notices(application)
    macos = Path(application) / "Contents/MacOS"
    records = embedded_manifest(macos / "zeta-desktop")
    if len(records) != 3 or records[0] != "minutus-evm-workers-v1":
        raise ValueError("Invalid EVM worker pins")
    for name, record in zip(NAMES, records[1:], strict=True):
        fields = record.split(" ")
        path = macos / name
        if len(fields) != 3 or fields[0] != name or path.is_symlink() or not path.is_file() or identity(path) != tuple(fields[1:]):
            raise ValueError("Bundled EVM worker differs from the host-build pin")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", default="-")
    verification = parser.add_mutually_exclusive_group()
    verification.add_argument("--verify-bundle", type=Path)
    verification.add_argument("--verify-notice-sources", action="store_true")
    args = parser.parse_args()
    try:
        if args.verify_notice_sources:
            verify_notice_sources(ROOT)
        elif args.verify_bundle:
            verify_bundle(args.verify_bundle)
        else:
            stage(args.identity)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"EVM worker staging stopped: {error}", file=sys.stderr)
        sys.exit(1)
