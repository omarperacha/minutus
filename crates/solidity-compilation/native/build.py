#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build the standalone compiler helper using macOS system frameworks only."""
import argparse
from pathlib import Path
import re
import subprocess


def build(destination, signing_identity="-", module_cache=None):
    if signing_identity != "-" and not re.fullmatch(r"[0-9a-fA-F]{40}", signing_identity):
        raise ValueError("Expected one exact signing certificate hash")
    if destination.is_symlink():
        raise ValueError("Compiler helper destination is indirect")
    source = Path(__file__).resolve().parent
    command = ["swiftc", "-O", "-target", "arm64-apple-macosx13.3"]
    if module_cache is not None:
        command += ["-module-cache-path", str(module_cache)]
    subprocess.run([*command, str(source / "compiler.swift"), "-o", str(destination)], check=True)
    command = ["/usr/bin/codesign", "--force", "--sign", signing_identity,
               "--identifier", "ai.minutus.minutus-solidity-compiler", "--digest-algorithm=sha256"]
    if signing_identity != "-":
        command += ["--timestamp", "--options", "runtime", "--entitlements",
                    str(source / "SolidityCompiler.entitlements.plist")]
    subprocess.run([*command, str(destination)], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--identity", default="-")
    parser.add_argument("--module-cache", type=Path)
    args = parser.parse_args()
    build(args.destination, args.identity, args.module_cache)
