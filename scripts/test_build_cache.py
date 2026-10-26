"""Use disposable fixtures to verify the build-cache budget and deletion boundary."""

import fcntl
import json
from pathlib import Path
import os
import tempfile
import unittest
from unittest.mock import patch

import build_cache
from build_cache import CacheBusyError, cache_size, maintain, profile_directories


class BuildCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="zeta-cache-policy-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.target = self.root / "target"
        self.profile = self.target / "debug"
        self.profile.mkdir(parents=True)
        (self.profile / ".cargo-lock").touch()

    def write(self, relative, size=4096):
        path = self.profile / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * size)
        return path

    def test_under_budget_preserves_warm_builds(self):
        dependency = self.write("deps/libcurrent.rlib")
        incremental = self.write("incremental/current/object.o")
        before = cache_size([self.profile])
        self.assertEqual(maintain(self.target, limit=before, retain=0), before)
        self.assertTrue(dependency.exists())
        self.assertTrue(incremental.exists())

    def test_evicts_old_incremental_units_before_dependencies(self):
        dependency = self.write("deps/libcurrent.rlib")
        old = self.write("incremental/old/object.o", 8192)
        new = self.write("incremental/new/object.o")
        os.utime(old.parent, (1, 1))
        before = cache_size([self.profile])
        remaining = maintain(self.target, limit=before - 1, retain=before - 4096)
        self.assertLess(remaining, before)
        self.assertFalse(old.exists())
        self.assertTrue(new.exists())
        self.assertTrue(dependency.exists())

    def test_oversize_dependencies_reset_consistent_cache_but_preserve_apps(self):
        for name in ("deps", "build", ".fingerprint"):
            self.write(f"{name}/entry")
        binary = self.write("zeta-desktop")
        app = self.write("bundle/macos/Minutus.app/Contents/MacOS/zeta-desktop")
        self.assertEqual(maintain(self.target, limit=1, retain=0), 0)
        self.assertTrue(binary.exists())
        self.assertTrue(app.exists())
        self.assertTrue((self.profile / ".cargo-lock").exists())
        for name in ("deps", "build", ".fingerprint"):
            self.assertFalse((self.profile / name).exists())

    def test_preserves_dependencies_when_incremental_eviction_meets_hard_limit(self):
        dependency = self.write("deps/libcurrent.rlib", 16384)
        incremental = self.write("incremental/old/object.o", 8192)
        dependency_size = cache_size([self.profile]) - build_cache.size_bytes(self.profile / "incremental")
        remaining = maintain(self.target, limit=dependency_size + 4096, retain=dependency_size - 4096)
        self.assertLessEqual(remaining, dependency_size + 4096)
        self.assertTrue(dependency.exists())
        self.assertFalse(incremental.exists())

    def test_busy_cargo_lock_prevents_all_deletion(self):
        dependency = self.write("deps/libcurrent.rlib")
        release = self.target / "release"
        release.mkdir()
        (release / ".cargo-lock").touch()
        with (release / ".cargo-lock").open("r+b") as handle:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(CacheBusyError):
                maintain(self.target, clear=True)
        self.assertTrue(dependency.exists())

    def test_symlinked_cache_is_rejected_without_touching_destination(self):
        outside = self.root / "outside"
        outside.mkdir()
        protected = outside / "keep"
        protected.write_text("keep")
        (self.profile / "deps").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlinked cache"):
            maintain(self.target, clear=True)
        self.assertEqual(protected.read_text(), "keep")

    def test_nested_symlinks_are_unlinked_without_following_them(self):
        outside = self.root / "outside"
        outside.mkdir()
        protected = outside / "keep"
        protected.write_text("keep")
        self.write("incremental/old/object.o")
        (self.profile / "incremental/old/link").symlink_to(outside, target_is_directory=True)
        maintain(self.target, clear=True)
        self.assertEqual(protected.read_text(), "keep")
        self.assertFalse((self.profile / "incremental").exists())

    def test_target_triple_profiles_are_included_but_payloads_are_not(self):
        cross = self.target / "aarch64-apple-darwin" / "release"
        cross.mkdir(parents=True)
        (cross / ".cargo-lock").touch()
        (cross / "deps").mkdir()
        (cross / "deps/cache").write_bytes(b"old")
        payload = self.target / "qwen-accel-runtime" / "runtime"
        payload.mkdir(parents=True)
        (payload / "keep").write_bytes(b"runtime")
        self.assertEqual(len(profile_directories(self.target)), 2)
        maintain(self.target, clear=True)
        self.assertFalse((cross / "deps").exists())
        self.assertTrue((payload / "keep").exists())

    def test_symlinked_target_is_rejected(self):
        alias = self.root / "alias"
        alias.symlink_to(self.target, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlinked Cargo target"):
            maintain(alias, clear=True)

    def test_custom_cargo_configuration_is_not_automatically_cleaned(self):
        for key in ("target_directory", "build_directory"):
            metadata = {name: str(build_cache.TARGET) for name in ("target_directory", "build_directory")}
            metadata[key] = str(self.root / "custom")
            with (
                patch.object(build_cache.subprocess, "run") as run,
                patch.object(build_cache, "maintain") as clean,
            ):
                run.return_value.stdout = json.dumps(metadata)
                build_cache.maintain_workspace()
            clean.assert_not_called()


if __name__ == "__main__":
    unittest.main()
