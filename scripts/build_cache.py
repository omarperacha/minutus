"""Bound this workspace's disposable Cargo cache without removing built apps.

Cargo 1.97 uses flock on each profile's .cargo-lock on Unix. Take those same
locks, non-blocking, before inspecting or deleting artifacts. Never remove lock
files: replacing one would let a concurrent Cargo process lock a different inode.
"""

from contextlib import ExitStack, contextmanager
import fcntl
import json
from pathlib import Path
import os
import shutil
import stat
import subprocess
import sys

REPOSITORY = Path(__file__).resolve().parent.parent
TARGET = REPOSITORY / "target"
MAX_CACHE_BYTES = 6_000_000_000
RETAIN_CACHE_BYTES = 5_000_000_000
CACHE_DIRECTORIES = ("incremental", "deps", "build", ".fingerprint")


class CacheBusyError(RuntimeError):
    """A Cargo build is using a cache which must not be modified."""


def profile_directories(target):
    """Find Cargo profiles at the normal or target-triple nesting level."""
    if target.is_symlink():
        raise ValueError("Refusing to maintain a symlinked Cargo target directory.")
    if not target.exists():
        return []
    profiles = []
    for entry in target.iterdir():
        if entry.is_symlink() or not entry.is_dir():
            continue
        if (entry / ".cargo-lock").exists():
            profiles.append(entry)
        elif entry.name not in {"doc", "package", "tmp", "cargo-timings"}:
            for child in entry.iterdir():
                if not child.is_symlink() and child.is_dir() and (child / ".cargo-lock").exists():
                    profiles.append(child)
    return sorted(profiles)


@contextmanager
def locked_profiles(target):
    """Acquire every known profile lock before making any cache changes."""
    profiles = profile_directories(target)
    with ExitStack() as stack:
        for profile in profiles:
            descriptor = os.open(profile / ".cargo-lock", os.O_RDWR | os.O_NOFOLLOW)
            handle = stack.enter_context(os.fdopen(descriptor, "r+b"))
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise ValueError("Cargo cache lock is not a regular file.")
            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise CacheBusyError(f"Cargo is using {profile}; cache cleanup deferred.") from error
        for profile in profiles:
            for name in CACHE_DIRECTORIES:
                if (profile / name).is_symlink():
                    raise ValueError(f"Refusing to follow a symlinked cache: {profile / name}")
        yield profiles


def size_bytes(path):
    """Count allocated bytes, without following links outside the cache."""
    if not path.exists():
        return 0
    total = path.lstat().st_blocks * 512
    for directory, directories, files in os.walk(path, followlinks=False):
        for name in directories + files:
            metadata = os.lstat(Path(directory) / name)
            total += metadata.st_blocks * 512
    return total


def cache_size(profiles):
    return sum(size_bytes(profile / name) for profile in profiles for name in CACHE_DIRECTORIES)


def remove_directory(path):
    if path.is_symlink():
        raise ValueError(f"Refusing to remove a symlinked cache: {path}")
    if path.exists():
        shutil.rmtree(path)


def maintain(target=TARGET, *, clear=False, limit=MAX_CACHE_BYTES, retain=RETAIN_CACHE_BYTES):
    """Trim oldest incremental units first, then whole consistent profile caches.

    Final binaries, bundles, model/runtime payloads and Cargo's lock files are
    outside the removal allowlist. A profile reset may cause a cold rebuild; it
    is preferable to mixing selectively deleted fingerprints and dependencies.
    """
    if limit <= 0 or retain < 0 or retain > limit:
        raise ValueError("Invalid cache budget.")
    with locked_profiles(target) as profiles:
        if clear:
            for profile in profiles:
                for name in CACHE_DIRECTORIES:
                    remove_directory(profile / name)
            print("Cleared Cargo compiler caches; built executables and app bundles retained.", file=sys.stderr, flush=True)
            return 0
        total = cache_size(profiles)
        if total <= limit:
            return total
        print(f"Cargo cache is {total / 1e9:.2f} GB; trimming to at most {limit / 1e9:.1f} GB…", file=sys.stderr, flush=True)
        incremental = []
        for profile in profiles:
            directory = profile / "incremental"
            if directory.exists():
                incremental.extend(entry for entry in directory.iterdir() if entry.is_dir() and not entry.is_symlink())
        for entry in sorted(incremental, key=lambda path: path.stat().st_mtime_ns):
            released = size_bytes(entry)
            remove_directory(entry)
            total -= released
            if total <= retain:
                break
        if total > limit:
            # When dependencies alone exceed the budget, remove the least recently
            # built profile as a unit. Cargo regenerates all its metadata next time.
            for profile in sorted(profiles, key=lambda path: (path / ".fingerprint").stat().st_mtime_ns if (path / ".fingerprint").exists() else 0):
                for name in CACHE_DIRECTORIES:
                    directory = profile / name
                    total -= size_bytes(directory)
                    remove_directory(directory)
                print(f"Reset {profile.name} compiler cache to meet the budget; its next build will be cold.", file=sys.stderr, flush=True)
                if total <= limit:
                    break
        remaining = cache_size(profiles)
        if remaining > limit:
            raise RuntimeError("Cargo cache could not be brought below its size limit.")
        print(f"Cargo compiler cache retained: {remaining / 1e9:.2f} GB.", file=sys.stderr, flush=True)
        return remaining


def maintain_workspace():
    """Only apply automatic eviction to this repository's default Cargo paths."""
    result = subprocess.run(
        ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
        cwd=REPOSITORY, check=True, capture_output=True, text=True,
    )
    metadata = json.loads(result.stdout)
    if any(Path(metadata[key]).resolve() != TARGET for key in ("target_directory", "build_directory")):
        print("Custom Cargo directories are outside Minutus's automatic 6 GB cache budget.", file=sys.stderr, flush=True)
        return None
    return maintain()
