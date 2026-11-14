import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

import check_contract_profiles as policy


class ContractProfilePolicy(unittest.TestCase):
    def test_checked_in_source_and_artifacts_match(self):
        self.assertEqual(policy.verify_profile()['profileCommitmentSha256'], policy.REVISION)

    def test_artifact_source_manifest_and_metadata_tampering_reject(self):
        for name in ['manifest.json', 'runtime.hex', 'creation.hex', 'abi.json',
                     'input.json', 'storage-layout.json', 'metadata.json',
                     'sources/MinutusQualifiedERC20.sol']:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / 'profile'
                shutil.copytree(policy.PROFILE, root)
                path = root / name
                path.write_bytes(path.read_bytes() + b' ')
                with self.assertRaises(ValueError):
                    policy.verify_profile(root)

    def test_changing_file_and_its_manifest_hash_cannot_replace_review_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'profile'
            shutil.copytree(policy.PROFILE, root)
            altered = root / 'creation.hex'
            altered.write_bytes(b'0x00\n')
            manifest = json.loads((root / 'manifest.json').read_bytes())
            manifest['filesSha256']['creation.hex'] = policy.sha(altered.read_bytes())
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                policy.verify_profile(root)


class CompilerDownloadPolicy(unittest.TestCase):
    def setUp(self):
        self.compiler = b'pinned compiler fixture'
        self.manifest = {
            'compiler': {'downloadUrl': 'https://binaries.soliditylang.org/pinned.js'},
            'compilerSha256': policy.sha(self.compiler),
        }

    def test_forbidden_primary_uses_exact_official_mirror(self):
        primary = self.manifest['compiler']['downloadUrl']
        forbidden = urllib.error.HTTPError(primary, 403, 'Forbidden', {}, None)
        with patch.object(policy.urllib.request, 'urlopen', side_effect=[
                forbidden, io.BytesIO(self.compiler)]) as download:
            self.assertEqual(policy.download_compiler(self.manifest), self.compiler)
        self.assertEqual(download.call_args_list[0].args, (primary,))
        self.assertEqual(download.call_args_list[1].args,
                         (policy.OFFICIAL_COMPILER_MIRROR,))
        self.assertEqual(download.call_args_list[1].kwargs, {'timeout': 30})

    def test_primary_success_does_not_contact_mirror(self):
        with patch.object(policy.urllib.request, 'urlopen',
                          return_value=io.BytesIO(self.compiler)) as download:
            self.assertEqual(policy.download_compiler(self.manifest), self.compiler)
        self.assertEqual(download.call_count, 1)

    def test_checksum_mismatch_fails_closed_on_either_source(self):
        for use_mirror in [False, True]:
            with self.subTest(use_mirror=use_mirror):
                responses = [io.BytesIO(b'altered compiler')]
                if use_mirror:
                    responses.insert(0, urllib.error.URLError('unavailable'))
                with patch.object(policy.urllib.request, 'urlopen',
                                  side_effect=responses) as download:
                    with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                        policy.download_compiler(self.manifest)
                self.assertEqual(download.call_count, 2 if use_mirror else 1)

    def test_oversized_response_fails_closed(self):
        with patch.object(policy, 'MAX_COMPILER_BYTES', 4), patch.object(
                policy.urllib.request, 'urlopen', return_value=io.BytesIO(b'12345')):
            with self.assertRaisesRegex(ValueError, 'download bound'):
                policy.download_compiler(self.manifest)

    def test_unavailable_sources_report_failure(self):
        with patch.object(policy.urllib.request, 'urlopen', side_effect=[
                urllib.error.URLError('unavailable'), TimeoutError('timed out')]):
            with self.assertRaisesRegex(RuntimeError, 'Pinned compiler unavailable'):
                policy.download_compiler(self.manifest)
