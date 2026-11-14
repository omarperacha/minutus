"""Exact notice source/bundle guards; no build, signing or network operations."""
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import tomllib

import evm_notices as notices


class NoticeTests(unittest.TestCase):
    def source_fixture(self, root):
        for _, source, _ in notices.NOTICES:
            target = root / source
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(notices.ROOT / source, target)
        shutil.copytree(notices.ROOT / "src-tauri" / notices.RESOURCE_NAME,
                        root / "src-tauri" / notices.RESOURCE_NAME)
        for name in ("tauri.conf.json", "tauri.evm-workers.conf.json"):
            shutil.copyfile(notices.ROOT / "src-tauri" / name, root / "src-tauri" / name)

    def bundle_fixture(self, root):
        application = root / "App.app"
        resource = application / "Contents/Resources" / notices.RESOURCE_NAME
        shutil.copytree(notices.ROOT / "src-tauri" / notices.RESOURCE_NAME, resource)
        return application, resource

    def test_reviewed_sources_copies_configs_and_profile_notice_are_bound(self):
        notices.verify_notice_sources()
        manifest = notices.ROOT / "src-tauri" / notices.RESOURCE_NAME / "manifest.sha256"
        self.assertEqual(hashlib.sha256(manifest.read_bytes()).hexdigest(), notices.MANIFEST_SHA256)
        profile = json.loads((notices.ROOT / "crates/chain-evm/profiles/erc20-oz-5.7.0/manifest.json").read_bytes())
        self.assertEqual(profile["filesSha256"]["OpenZeppelin-MIT.txt"], notices.NOTICES[0][2])
        for name in ("tauri.conf.json", "tauri.evm-workers.conf.json"):
            resources = json.loads((notices.ROOT / "src-tauri" / name).read_bytes())["bundle"]["resources"]
            self.assertEqual(resources[:4], ["ollama-runtime", "computation-service", "speech-runtime", "../LICENSE"])
            self.assertEqual(resources.count(notices.RESOURCE_NAME), 1)

    def test_direct_family_attribution_matches_exact_registry_archive_versions(self):
        packages = tomllib.loads((notices.ROOT / "Cargo.lock").read_text())["package"]
        attribution = (notices.ROOT / "docs/third-party/local-simulation/README.md").read_text()
        for name, version in (("revm", "42.0.1"), ("revm", "43.0.3"),
                              ("alloy-primitives", "1.7.3"), ("alloy-dyn-abi", "1.7.3"),
                              ("alloy-consensus", "2.5.0")):
            matches = [package for package in packages if package["name"] == name and package["version"] == version]
            self.assertEqual(len(matches), 1)
            self.assertIn(f"| {name} | {version} | `{matches[0]['checksum']}`", attribution)

    def test_missing_or_tampered_bundle_notices_and_manifest_reject(self):
        with tempfile.TemporaryDirectory() as directory:
            application, resource = self.bundle_fixture(Path(directory))
            notices.verify_bundled_notices(application)
            for destination, _, _ in notices.NOTICES:
                with self.subTest(notice=destination):
                    path = resource / destination
                    original = path.read_bytes()
                    path.unlink()
                    with self.assertRaises(ValueError):
                        notices.verify_bundled_notices(application)
                    path.write_bytes(original + b"tampered")
                    with self.assertRaises(ValueError):
                        notices.verify_bundled_notices(application)
                    path.write_bytes(original)
            manifest = resource / "manifest.sha256"
            manifest.write_bytes(manifest.read_bytes() + b"unreviewed notice\n")
            with self.assertRaisesRegex(ValueError, "manifest"):
                notices.verify_bundled_notices(application)

    def test_indirect_notice_leaf_parent_and_extra_file_reject(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            application, resource = self.bundle_fixture(root)
            leaf = resource / "local-simulation/Base-MIT.txt"
            original = leaf.read_bytes()
            leaf.unlink()
            leaf.symlink_to(notices.ROOT / "docs/third-party/local-simulation/Base-MIT.txt")
            with self.assertRaisesRegex(ValueError, "indirect"):
                notices.verify_bundled_notices(application)
            leaf.unlink()
            leaf.write_bytes(original)
            local = resource / "local-simulation"
            local.rename(root / "outside")
            local.symlink_to(root / "outside", target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "indirect"):
                notices.verify_bundled_notices(application)
            local.unlink()
            (root / "outside").rename(local)
            (resource / "unreviewed.txt").write_text("extra")
            with self.assertRaisesRegex(ValueError, "file set"):
                notices.verify_bundled_notices(application)

    def test_source_and_copy_cannot_be_jointly_changed_or_unbundled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.source_fixture(root)
            notices.verify_notice_sources(root)
            destination, source, _ = notices.NOTICES[0]
            original = (root / source).read_bytes()
            (root / source).write_bytes(original + b"changed")
            (root / "src-tauri" / notices.RESOURCE_NAME / destination).write_bytes(original + b"changed")
            with self.assertRaisesRegex(ValueError, "source notice"):
                notices.verify_notice_sources(root)
            (root / source).write_bytes(original)
            (root / "src-tauri" / notices.RESOURCE_NAME / destination).write_bytes(original)
            for name in ("tauri.conf.json", "tauri.evm-workers.conf.json"):
                path = root / "src-tauri" / name
                original_config = path.read_bytes()
                config = json.loads(original_config)
                config["bundle"]["resources"].remove(notices.RESOURCE_NAME)
                path.write_text(json.dumps(config))
                with self.assertRaisesRegex(ValueError, "not bundled"):
                    notices.verify_notice_sources(root)
                path.write_bytes(original_config)


if __name__ == "__main__":
    unittest.main()
