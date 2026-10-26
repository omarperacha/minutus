#!/usr/bin/env python3
"""Inspect, trim or clear Zeta's compiler cache; never removes built apps."""

import argparse
import sys

sys.dont_write_bytecode = True
from build_cache import CacheBusyError, TARGET, cache_size, locked_profiles, maintain


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("status", "trim", "clear"), default="status", nargs="?")
    args = parser.parse_args()
    try:
        if args.action == "status":
            with locked_profiles(TARGET) as profiles:
                print(f"Cargo compiler cache: {cache_size(profiles) / 1e9:.2f} GB (limit: 6 GB).")
        else:
            maintain(clear=args.action == "clear")
    except (CacheBusyError, OSError, ValueError, RuntimeError) as error:
        print(f"Cache maintenance stopped: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
