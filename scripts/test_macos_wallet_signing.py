"""Pure metadata tests: no signing, Keychain access, profiles or wallet secrets."""

import datetime
import hashlib
import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("wallet_signing", Path(__file__).with_name("macos-wallet-signing.py"))
signing = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(signing)
NOW = datetime.datetime(2026, 9, 3)


def profile():
    return {
        "Platform": ["OSX"],
        "ExpirationDate": NOW + datetime.timedelta(days=1),
        "TeamIdentifier": ["TESTTEAM"],
        "ApplicationIdentifierPrefix": ["TESTPREFIX"],
        "Entitlements": {
            signing.APPLICATION_ID: "TESTPREFIX.com.omarperacha.zeta",
            signing.TEAM_ID: "TESTTEAM",
            "keychain-access-groups": ["TESTPREFIX.*"],
        },
        "DeveloperCertificates": [b"test-certificate-metadata"],
    }


class WalletSigningTests(unittest.TestCase):
    def test_macos_26_entitlement_tree_is_parsed_strictly(self):
        output = """[Dict]
\t[Key] com.apple.application-identifier
\t[Value]
\t\t[String] TESTPREFIX.com.omarperacha.zeta
\t[Key] com.apple.security.app-sandbox
\t[Value]
\t\t[Bool] true
"""
        self.assertEqual(
            signing.parse_entitlement_tree(output),
            {
                "com.apple.application-identifier": "TESTPREFIX.com.omarperacha.zeta",
                "com.apple.security.app-sandbox": True,
            },
        )
        for malformed in ["", "[Array]\n", output + "\t[Key] duplicate\n"]:
            with self.subTest(malformed=malformed), self.assertRaises(ValueError):
                signing.parse_entitlement_tree(malformed)

    def test_explicit_profile_supports_distinct_team_and_application_prefix(self):
        self.assertEqual(signing.profile_identity(profile(), NOW), ("TESTPREFIX.com.omarperacha.zeta", "TESTTEAM"))

    def test_rejects_expired_wrong_platform_and_inconsistent_team(self):
        for field, value in [
            ("ExpirationDate", NOW),
            ("Platform", ["iOS"]),
            ("TeamIdentifier", ["OTHER"]),
            ("ApplicationIdentifierPrefix", ["OTHER"]),
        ]:
            invalid = profile()
            invalid[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                signing.profile_identity(invalid, NOW)

    def test_rejects_missing_wildcard_and_foreign_application_identity(self):
        for app_id in ["", "TESTPREFIX.*", "TESTPREFIX.com.other.app"]:
            invalid = profile()
            invalid["Entitlements"][signing.APPLICATION_ID] = app_id
            with self.subTest(app_id=app_id), self.assertRaises(ValueError):
                signing.profile_identity(invalid, NOW)

    def test_certificate_must_be_both_installed_and_authorized(self):
        identity = hashlib.sha1(profile()["DeveloperCertificates"][0]).hexdigest().upper()
        installed = f'  1) {identity} "Apple Development: Test"\n  1 valid identities found'
        signing.validate_certificate(profile(), identity, installed)
        with self.assertRaises(ValueError):
            signing.validate_certificate(profile(), identity, "0 valid identities found")
        foreign = profile()
        foreign["DeveloperCertificates"] = [b"other-certificate"]
        with self.assertRaises(ValueError):
            signing.validate_certificate(foreign, identity, installed)
        for invalid in ["-", "Apple Development: Test", "bad-hash"]:
            with self.subTest(identity=invalid), self.assertRaises(ValueError):
                signing.validate_certificate(profile(), invalid, installed)

    def test_generated_entitlements_do_not_grant_shared_keychain_access(self):
        base = {"com.apple.security.app-sandbox": True}
        actual = signing.wallet_entitlements(base, "TESTPREFIX.com.omarperacha.zeta", "TESTTEAM")
        self.assertNotIn("keychain-access-groups", actual)
        self.assertNotIn("com.apple.security.get-task-allow", actual)
        self.assertEqual(base, {"com.apple.security.app-sandbox": True})
        with self.assertRaises(ValueError):
            signing.wallet_entitlements({"keychain-access-groups": []}, "app", "team")

    def test_verifier_requires_an_apple_signature_from_a_profile_certificate(self):
        requirement = signing.verification_requirement(profile())
        self.assertTrue(requirement.startswith('=anchor apple generic and identifier "com.omarperacha.zeta"'))
        digest = hashlib.sha1(profile()["DeveloperCertificates"][0]).hexdigest()
        self.assertIn(f'certificate leaf = H"{digest}"', requirement)
        invalid = profile()
        invalid["DeveloperCertificates"] = []
        with self.assertRaises(ValueError):
            signing.verification_requirement(invalid)

    def test_staging_requires_its_own_explicit_profile(self):
        current = profile()
        with self.assertRaisesRegex(ValueError, "explicit provisioning profile"):
            signing.profile_identity(current, NOW, signing.STAGING_BUNDLE_ID)
        current["Entitlements"][signing.APPLICATION_ID] = "TESTPREFIX." + signing.STAGING_BUNDLE_ID
        self.assertEqual(signing.profile_identity(current, NOW, signing.STAGING_BUNDLE_ID)[0],
                         "TESTPREFIX." + signing.STAGING_BUNDLE_ID)
        self.assertIn(signing.STAGING_BUNDLE_ID, signing.verification_requirement(current, signing.STAGING_BUNDLE_ID))
        with self.assertRaises(ValueError):
            signing.profile_identity(current, NOW)

    def test_missing_identity_stops_before_modifying_the_app(self):
        current = profile()
        current["ExpirationDate"] = datetime.datetime.now() + datetime.timedelta(days=1)
        identity = hashlib.sha1(current["DeveloperCertificates"][0]).hexdigest().upper()
        with tempfile.TemporaryDirectory(prefix="zeta-signing-test-") as directory:
            root = Path(directory)
            profile_path = root / "Test.provisionprofile"
            profile_path.write_bytes(b"synthetic-profile-placeholder")
            app = root / "Minutus.app"
            (app / "Contents").mkdir(parents=True)
            (app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": signing.BUNDLE_ID}))
            with patch.object(signing, "capture", side_effect=[plistlib.dumps(current), b"0 valid identities found"]), patch.object(signing.subprocess, "run") as run:
                with self.assertRaisesRegex(ValueError, "not installed"):
                    signing.sign(app, identity, profile_path)
                run.assert_not_called()
                self.assertFalse((app / "Contents/embedded.provisionprofile").exists())


if __name__ == "__main__":
    unittest.main()
