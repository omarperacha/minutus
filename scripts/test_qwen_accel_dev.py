import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('qwen_accel_dev', Path(__file__).with_name('qwen_accel_dev.py'))
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)


class ArtifactAdmission(unittest.TestCase):
    def fixture(self, root):
        names = ('ollama', 'llama-server', 'serve.py', 'upstream.lock.json')
        for name in names:
            (root/name).write_text(name)
        data = dict(schema='qwen-metal-artifact-v1', platform='darwin-arm64',
            ollama_version='0.32.14', status='experimental', patch_sha256='patch',
            files={name:dev.digest(root/name) for name in names})
        (root/'artifact.json').write_text(json.dumps(data))

    def test_tampered_or_extra_payload_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.fixture(root)
            dev.inspect(root)
            (root/'undeclared-runner').write_text('unreviewed')
            with self.assertRaises(ValueError): dev.inspect(root)
            (root/'undeclared-runner').unlink()
            (root/'llama-server').write_text('changed')
            with self.assertRaises(ValueError): dev.inspect(root)

    def test_manifest_replacement_requires_explicit_repin(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory); root = parent/'artifact'; root.mkdir(); self.fixture(root)
            lock = parent/'pin.json'; lock.write_text(json.dumps(dev.inspect(root)))
            dev.verify_pin(root, lock)
            data = json.loads((root/'artifact.json').read_text()); data['patch_sha256']='replacement'
            (root/'artifact.json').write_text(json.dumps(data))
            with self.assertRaises(ValueError): dev.verify_pin(root, lock)

    def test_symlink_payload_is_rejected_even_with_matching_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            parent=Path(directory);root=parent/'artifact';root.mkdir();self.fixture(root)
            original=root/'serve.py';target=parent/'runner';original.rename(target);original.symlink_to(target)
            with self.assertRaises(ValueError):dev.inspect(root)


if __name__ == '__main__':
    unittest.main()
