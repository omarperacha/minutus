import json
from pathlib import Path
import tempfile
import unittest

from qwen_accel_dev import digest, inspect
from stage_qwen_acceleration import prepare
import test_qwen_accel_runtime


class BundledAcceleration(unittest.TestCase):
    def fixture(self, root):
        artifact, bundled = test_qwen_accel_runtime.RuntimeComposition().fixture(root)
        (bundled/'llama-server').write_text('original backend')
        (bundled/'payload.sha256').write_text(''.join(
            f'{digest(p)}  ./{p.relative_to(bundled)}\n'
            for p in bundled.rglob('*') if p.is_file() and p.name != 'payload.sha256'))
        pin = root/'pin.json'
        pin.write_text(json.dumps(inspect(artifact)))
        return artifact, bundled, pin

    def test_staging_preserves_stock_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact, bundled, pin = self.fixture(Path(directory))
            original = digest(bundled/'llama-server')
            record = prepare(artifact, bundled, pin)
            self.assertEqual(digest(bundled/'llama-server'), original)
            self.assertEqual(digest(bundled/'qwen-accel/llama-server'), digest(artifact/'llama-server'))
            self.assertEqual(record['source_artifact_sha256'], digest(artifact/'artifact.json'))
            self.assertEqual(prepare(artifact, bundled, pin), record)
            self.assertFalse((bundled/'qwen-accel/serve.py').exists())
            self.assertFalse((bundled/'qwen-accel/artifact.json').exists())

    def test_tampered_stock_cannot_be_blessed_by_staging(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact, bundled, pin = self.fixture(Path(directory))
            (bundled/'llama-server').write_text('replacement')
            with self.assertRaises(ValueError):
                prepare(artifact, bundled, pin)
            self.assertFalse((bundled/'qwen-accel').exists())

    def test_stale_acceleration_requires_explicit_upgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact, bundled, pin = self.fixture(Path(directory))
            prepare(artifact, bundled, pin)
            (bundled/'qwen-accel/llama-server').write_text('replacement')
            with self.assertRaises(ValueError):
                prepare(artifact, bundled, pin)


if __name__ == '__main__':
    unittest.main()
