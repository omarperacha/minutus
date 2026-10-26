"""Negative packaging checks using small independent filesystem fixtures."""

from pathlib import Path
import tempfile
import unittest

from inspect_macos_release import digest, safe_files, verify_manifest


class PackageInspectionTests(unittest.TestCase):
    def test_rejects_weights_by_suffix_and_gguf_magic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'model.safetensors'
            path.write_bytes(b'fixture')
            with self.assertRaisesRegex(ValueError, 'Model weights'):
                safe_files(root)
            path.unlink()
            (root / 'innocent.dat').write_bytes(b'GGUFfixture')
            with self.assertRaisesRegex(ValueError, 'Model weights'):
                safe_files(root)

    def test_rejects_external_links_and_model_cache_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = root / 'bundle'
            bundle.mkdir()
            outside = root / 'outside'
            outside.write_bytes(b'fixture')
            link = bundle / 'linked'
            link.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, 'Unsafe bundle link'):
                safe_files(bundle)
            link.unlink()
            (bundle / 'blobs').mkdir()
            with self.assertRaisesRegex(ValueError, 'Model storage directory'):
                safe_files(bundle)

    def test_manifest_rejects_tampering_and_extra_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = root / 'runtime'
            payload.mkdir()
            executable = payload / 'llama-server'
            executable.write_bytes(b'independent fixture')
            manifest = root / 'trusted.sha256'
            manifest.write_text(f'{digest(executable)}  ./llama-server\n')
            (payload / 'payload.sha256').write_bytes(manifest.read_bytes())
            verify_manifest(payload, manifest)
            (payload / 'extra').write_bytes(b'extra')
            with self.assertRaisesRegex(ValueError, 'file set'):
                verify_manifest(payload, manifest)
            (payload / 'extra').unlink()
            executable.write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                verify_manifest(payload, manifest)

    def test_manifest_rejects_parent_path_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / 'trusted.sha256'
            manifest.write_text(f'{"0" * 64}  ../outside\n')
            with self.assertRaisesRegex(ValueError, 'manifest path'):
                verify_manifest(root, manifest)


if __name__ == '__main__':
    unittest.main()
