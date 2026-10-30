import json
from pathlib import Path
import shutil
import tempfile
import unittest

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
