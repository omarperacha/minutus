#!/usr/bin/env python3
"""Maintain the compiler cache after a successful build, then replace this process."""

import os
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True
from build_cache import CacheBusyError, maintain_workspace
from sign_dev_ollama_runtime import sign_runtime


def main():
    if len(sys.argv) < 2:
        print("Expected a Cargo-built executable.", file=sys.stderr)
        return 1
    try:
        maintain_workspace()
    except CacheBusyError as error:
        print(str(error), file=sys.stderr)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Cargo cache maintenance failed: {error}", file=sys.stderr)
        return 1
    if "ZETA_DEV_OLLAMA_ADDRESS" not in os.environ:
        try:
            repaired = sign_runtime(Path(sys.argv[1]).resolve().parent / "ollama-runtime")
            if repaired:
                print(f"Repaired {repaired} copied Ollama runtime signatures for development.")
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(f"Ollama development signing failed: {error}", file=sys.stderr)
            return 1
    os.execv(sys.argv[1], sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
