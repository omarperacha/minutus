import json
from pathlib import Path
import tempfile
import unittest

from qwen_accel_dev import digest, inspect
from qwen_accel_runtime import stage
import test_qwen_accel_dev


class RuntimeComposition(unittest.TestCase):
    def fixture(self, root):
        artifact = root/'artifact'; artifact.mkdir()
        test_qwen_accel_dev.ArtifactAdmission().fixture(artifact)
        bundled = root/'bundled'; bundled.mkdir()
        (bundled/'ollama').write_text('ollama')
        for variant in ('mlx_metal_v3', 'mlx_metal_v4'):
            (bundled/variant).mkdir()
            for name in ('libmlx.dylib', 'libmlxc.dylib', 'mlx.metallib'):
                (bundled/variant/name).write_text(variant+name)
        (bundled/'payload.sha256').write_text(''.join(
            f'{digest(p)}  ./{p.relative_to(bundled)}\n'
            for p in bundled.rglob('*') if p.is_file()))
        return artifact, bundled

    def test_composition_preserves_original_and_verifies_added_libraries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); artifact, bundled = self.fixture(root)
            original = digest(artifact/'artifact.json')
            staged = stage(artifact, bundled, root/'run', digest, inspect)
            inspect(staged)
            data = json.loads((staged/'artifact.json').read_text())
            self.assertEqual(data['composition']['experimental_artifact_sha256'], original)
            self.assertEqual(digest(artifact/'artifact.json'), original)
            self.assertEqual(digest(staged/'llama-server'), digest(artifact/'llama-server'))
            self.assertEqual(stage(artifact, bundled, root/'run', digest, inspect), staged)
            (staged/'mlx_metal_v3/libmlx.dylib').write_text('tampered')
            with self.assertRaises(ValueError): inspect(staged)
            # Updating the cache's self-reported digest cannot bypass its input pins.
            data['files']['mlx_metal_v3/libmlx.dylib'] = digest(staged/'mlx_metal_v3/libmlx.dylib')
            (staged/'artifact.json').write_text(json.dumps(data))
            with self.assertRaises(ValueError): stage(artifact, bundled, root/'run', digest, inspect)

    def test_corrupt_library_and_different_provider_are_rejected_before_staging(self):
        for relative in ('mlx_metal_v3/libmlx.dylib', 'ollama'):
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); artifact, bundled = self.fixture(root)
                (bundled/relative).write_text('changed')
                with self.assertRaises(ValueError): stage(artifact, bundled, root/'run', digest, inspect)
                self.assertFalse((root/'run').exists())


if __name__ == '__main__':
    unittest.main()
