#!/usr/bin/env python3
"""Launch Tauri development with optional local x402 debug configuration."""

from pathlib import Path
import shutil
import subprocess
import sys

from x402_debug_env import environment


REPOSITORY = Path(__file__).resolve().parent.parent


def main() -> int:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        print("pnpm is unavailable on PATH.", file=sys.stderr)
        return 1
    try:
        subprocess.run(
            [
                pnpm, "tauri", "dev", "--config", "src-tauri/tauri.dev.conf.json",
                "--runner", str(REPOSITORY / "scripts/cargo-dev"),
                *sys.argv[1:],
            ],
            cwd=REPOSITORY,
            env=environment(REPOSITORY),
            check=True,
        )
    except (OSError, UnicodeError, ValueError) as error:
        print(f"Tauri development stopped: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
