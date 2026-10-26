"""Authentication and extraction rejection tests for the public source bootstrap."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import json
from unittest.mock import patch
import build_macos_release
from bootstrap import extract, fetch


class BootstrapTests(unittest.TestCase):
    def test_changed_cached_archive_is_rejected_without_network(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'source.tar.gz'
            path.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'cached archive digest mismatch'):
                fetch('https://invalid.example/source.tar.gz', path, hashlib.sha256(b'original').hexdigest())

    def test_traversal_and_links_rejected_before_destination_created(self):
        for name, kind in [('../escape', tarfile.REGTYPE), ('root/link', tarfile.SYMTYPE), ('root/device', tarfile.CHRTYPE)]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                archive = Path(tmp) / 'source.tar.gz'
                with tarfile.open(archive, 'w:gz') as out:
                    item = tarfile.TarInfo(name)
                    item.type = kind
                    item.linkname = '/outside'
                    out.addfile(item)
                destination = Path(tmp) / 'extract'
                with self.assertRaisesRegex(ValueError, 'unsafe source archive member'):
                    extract(archive, destination)
                self.assertFalse(destination.exists())

    def test_official_signing_rejects_unreviewed_source_build_before_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'src-tauri/ollama-runtime/qwen-accel').mkdir(parents=True)
            (root / 'scripts/qwen_accel_bundled.lock.json').write_text(json.dumps({'artifact_sha256': 'reviewed'}))
            (root / 'src-tauri/ollama-runtime/qwen-accel/payload.json').write_text(json.dumps({'source_artifact_sha256': 'rebuilt'}))
            with patch.object(build_macos_release, 'ROOT', root), patch('sys.argv', ['release', '--distribution-config', 'unused.json']):
                with self.assertRaisesRegex(ValueError, 'Official signing requires'):
                    build_macos_release.main()

    def test_regular_single_root_source_extracts(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / 'source.tar.gz'
            with tarfile.open(archive, 'w:gz') as out:
                item = tarfile.TarInfo('root/source.py')
                item.size = 4
                out.addfile(item, io.BytesIO(b'code'))
            self.assertEqual((extract(archive, Path(tmp) / 'extract') / 'source.py').read_bytes(), b'code')


if __name__ == '__main__':
    unittest.main()
