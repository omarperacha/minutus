#!/usr/bin/env python3
"""Offline mutations of the production native XPC reply/descriptor parser."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


@unittest.skipUnless(sys.platform == "darwin", "macOS XPC dictionary API required")
class ClientProtocolTests(unittest.TestCase):
    def test_control_receipts_pipe_ownership_and_original_service_end(self):
        with tempfile.TemporaryDirectory(prefix="minutus-worker-client-test-") as temporary:
            executable = Path(temporary) / "client-tests"
            subprocess.run([
                "xcrun", "clang", "-arch", "arm64", "-mmacosx-version-min=13.3",
                "-std=c11", "-fblocks", "-Wall", "-Wextra", "-Werror",
                str(ROOT / "crates/worker-transport/native/tests/client.c"),
                "-o", str(executable),
            ], check=True, timeout=30, capture_output=True)
            result = subprocess.run([str(executable)], check=True, timeout=10,
                                    capture_output=True, text=True)
            self.assertEqual(result.stdout,
                             "worker transport private protocol mutations passed\n")
            self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
