"""Pin validation tests use synthetic files; no build, signing or wallet access."""
import importlib.util
from pathlib import Path
import tempfile
import struct
import shutil
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("stage_evm_workers", Path(__file__).with_name("stage_evm_workers.py"))
workers = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workers)


class WorkerPinTests(unittest.TestCase):
    def test_bundle_rejects_replacement_and_indirect_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pins = root / "src-tauri/evm-workers"
            pins.mkdir(parents=True)
            macos = root / "App.app/Contents/MacOS"
            macos.mkdir(parents=True)
            shutil.copytree(workers.ROOT / "src-tauri/third-party-notices", root / "App.app/Contents/Resources/third-party-notices")
            records = ["minutus-evm-workers-v1"]
            for name in workers.NAMES:
                (macos / name).write_bytes(b"reviewed")
                records.append(f"{name} {'a' * 64} {'b' * 40}")
            (pins / "manifest.txt").write_text("\n".join(records) + "\n")
            with patch.object(workers, "embedded_manifest", return_value=records), patch.object(workers, "identity", return_value=("a" * 64, "b" * 40)):
                workers.verify_bundle(root / "App.app")
                notice = root / "App.app/Contents/Resources/third-party-notices/local-simulation/Base-MIT.txt"
                notice_bytes = notice.read_bytes()
                notice.unlink()
                with self.assertRaisesRegex(ValueError, "notice"):
                    workers.verify_bundle(root / "App.app")
                notice.write_bytes(notice_bytes)
                with patch.object(workers, "identity", return_value=("c" * 64, "b" * 40)):
                    with self.assertRaises(ValueError):
                        workers.verify_bundle(root / "App.app")
                helper = macos / workers.NAMES[0]
                helper.unlink()
                helper.symlink_to(macos / workers.NAMES[1])
                with self.assertRaises(ValueError):
                    workers.verify_bundle(root / "App.app")

    def test_embedded_manifest_is_read_from_exact_bounded_host_section(self):
        text = b"minutus-evm-workers-v1\nknown\n"
        segment = struct.pack("<II16sQQQQiiII", 0x19, 152, b"__TEXT", 0, 0, 0, 0, 7, 5, 1, 0)
        section = struct.pack("<16s16sQQIIIIIIII", b"__evm_workers", b"__TEXT", 0, len(text), 184, 0, 0, 0, 0, 0, 0, 0)
        header = struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 152, 0, 0)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "host"
            path.write_bytes(header + segment + section + text)
            self.assertEqual(workers.embedded_manifest(path), text.decode().splitlines())
            for malformed in (b"", header + segment + section, b"bad!" + (header + segment + section + text)[4:]):
                path.write_bytes(malformed)
                with self.assertRaises(ValueError):
                    workers.embedded_manifest(path)

    def test_stage_rejects_dynamic_identity_before_running_build(self):
        with patch.object(workers.sys, "platform", "darwin"), patch.object(workers.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                workers.stage("arbitrary identity name")
            run.assert_not_called()

    def test_stage_rejects_missing_notices_before_running_build(self):
        with patch.object(workers.sys, "platform", "darwin"), patch.object(workers.subprocess, "run") as run, patch.object(workers, "verify_notice_sources", side_effect=ValueError("missing reviewed notices")):
            with self.assertRaisesRegex(ValueError, "notices"):
                workers.stage()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
