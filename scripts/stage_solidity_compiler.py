#!/usr/bin/env python3
"""Build/sign the authority-free JavaScriptCore compiler before host pinning."""
import argparse
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import sys

from stage_evm_workers import embedded_manifest, identity
from evm_notices import verify_bundled_notices, verify_notice_sources

ROOT = Path(__file__).resolve().parent.parent
NAME = "minutus-solidity-compiler"
TARGET = "aarch64-apple-darwin"


def executable_commands(path):
    """Read only a bounded regular thin arm64 executable's load commands."""
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or not 32 <= metadata.st_size <= 16 * 1024 * 1024:
            raise ValueError("Invalid compiler helper file")
        header = stream.read(32)
        magic, cpu, subtype, kind, count, size, _, _ = struct.unpack("<IiiIIIII", header)
        if (magic != 0xFEEDFACF or cpu != 0x0100000C or subtype != 0 or kind != 2
                or not 0 < count <= 4096 or size > 4 * 1024 * 1024):
            raise ValueError("Compiler helper must be a thin arm64 executable")
        commands = stream.read(size)
    if len(commands) != size:
        raise ValueError("Truncated compiler load commands")
    offset, result = 0, []
    for _ in range(count):
        if offset + 8 > size:
            raise ValueError("Truncated compiler load command")
        command, length = struct.unpack_from("<II", commands, offset)
        if length < 8 or length % 8 or offset + length > size:
            raise ValueError("Invalid compiler load command")
        result.append((command, commands[offset:offset + length]))
        offset += length
    if offset != size:
        raise ValueError("Compiler load command size mismatch")
    return result


def command_path(command, minimum):
    if len(command) < minimum:
        raise ValueError("Truncated compiler dependency command")
    offset = struct.unpack_from("<I", command, 8)[0]
    if not minimum <= offset < len(command):
        raise ValueError("Invalid compiler dependency name")
    name, separator, _ = command[offset:].partition(b"\0")
    if not separator:
        raise ValueError("Unterminated compiler dependency name")
    return name.decode("ascii")


def system_path(path):
    return path.startswith(("/usr/lib/", "/System/Library/")) and ".." not in Path(path).parts


def verify_executable(path):
    """Enforce the actual deployment target and system-only loader surface."""
    versions, dependencies = 0, 0
    for command, value in executable_commands(path):
        if command == 0x32:  # LC_BUILD_VERSION
            if len(value) < 24:
                raise ValueError("Truncated compiler build version")
            platform, minimum, _, tools = struct.unpack_from("<IIII", value, 8)
            if platform != 1 or minimum != (13 << 16 | 3 << 8) or len(value) != 24 + tools * 8:
                raise ValueError("Compiler helper must target macOS 13.3")
            versions += 1
        elif command in (0x24, 0x25, 0x2F, 0x30, 0x27, 0xD):
            raise ValueError("Unsupported compiler platform or loader override")
        elif command in (0xC, 0x80000018, 0x8000001F, 0x20, 0x80000023):
            if not system_path(command_path(value, 24)):
                raise ValueError("Compiler helper has a non-system dynamic dependency")
            dependencies += 1
        elif command == 0x8000001C and not system_path(command_path(value, 12)):
            raise ValueError("Compiler helper has a non-system runtime search path")
        elif command == 0xE and command_path(value, 12) != "/usr/lib/dyld":
            raise ValueError("Compiler helper has a non-system loader")
    if versions != 1 or dependencies == 0:
        raise ValueError("Missing or ambiguous compiler deployment/dependency information")


def stage(signing_identity="-"):
    if signing_identity != "-" and not re.fullmatch(r"[0-9a-fA-F]{40}", signing_identity):
        raise ValueError("Expected one exact signing certificate hash")
    verify_notice_sources()
    destination = ROOT / "src-tauri/binaries" / f"{NAME}-{TARGET}"
    if destination.is_symlink():
        raise ValueError("Compiler helper destination is indirect")
    subprocess.run([sys.executable, str(ROOT / "crates/solidity-compilation/native/build.py"),
                    str(destination), "--identity", signing_identity,
                    "--module-cache", str(ROOT / "target/solidity-swift-cache")],
                   cwd=ROOT, check=True)
    verify_executable(destination)
    digest, cdhash = identity(destination)
    directory = ROOT / "src-tauri/solidity-compiler"
    directory.mkdir(exist_ok=True)
    temporary = directory / "manifest.txt.incomplete"
    temporary.write_text(f"minutus-solidity-compiler-v1\n{NAME} {digest} {cdhash}\n", encoding="ascii")
    temporary.replace(directory / "manifest.txt")


def verify_bundle(application):
    # The shared bounded Mach-O reader takes a section name; source-side mutable
    # manifests never substitute for the host's actual embedded identity.
    macos = application / "Contents/MacOS"
    records = embedded_manifest(macos / "zeta-desktop", section_name=b"__solc_worker")
    if len(records) != 2 or records[0] != "minutus-solidity-compiler-v1":
        raise ValueError("Invalid compiler helper pins")
    fields = records[1].split(" ")
    helper = macos / NAME
    verify_executable(helper)
    if len(fields) != 3 or fields[0] != NAME or helper.is_symlink() or identity(helper) != tuple(fields[1:]):
        raise ValueError("Bundled compiler helper differs from host-build pins")
    verify_bundled_notices(application)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", default="-")
    parser.add_argument("--verify-bundle", type=Path)
    args = parser.parse_args()
    if args.verify_bundle:
        verify_bundle(args.verify_bundle)
    else:
        stage(args.identity)
