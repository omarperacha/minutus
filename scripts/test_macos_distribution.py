"""Pure release-policy tests; never access real keys or submit to Apple."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import macos_distribution as release

IDENTITY = 'A' * 40
DETAILS = 'CodeDirectory v=20500 flags=0x10000(runtime) hashes=1\nAuthority=Developer ID Application: Test\nTeamIdentifier=TESTTEAM\nTimestamp=Oct 6, 2026 at 12:00:00\n'


class DistributionTests(unittest.TestCase):
    def test_only_exact_developer_id_application_identity_is_accepted(self):
        release.require_developer_id(IDENTITY, f'1) {IDENTITY} "Developer ID Application: Test"')
        for label in ['Apple Development: Test', 'Apple Distribution: Test', 'Developer ID Installer: Test']:
            with self.subTest(label=label), self.assertRaises(ValueError):
                release.require_developer_id(IDENTITY, f'1) {IDENTITY} "{label}"')
        with self.assertRaises(ValueError):
            release.require_developer_id('B' * 40, f'1) {IDENTITY} "Developer ID Application: Test"')

    def test_runtime_requires_hardening_team_and_secure_timestamp(self):
        release.require_runtime_details(DETAILS, 'TESTTEAM')
        for invalid in [DETAILS.replace('0x10000(runtime)', '0x0(none)'),
                        DETAILS.replace('Developer ID Application:', 'Apple Development:'),
                        DETAILS.replace('TESTTEAM', 'OTHERTEAM'),
                        DETAILS.replace('Timestamp=Oct 6, 2026 at 12:00:00', 'Timestamp=none'),
                        DETAILS.replace('Timestamp=', 'Signed Time=')]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                release.require_runtime_details(invalid, 'TESTTEAM')

    def test_config_rejects_passwords_wildcard_id_and_relative_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'release.json'
            valid = dict(identity=IDENTITY, profile='/tmp/Test.provisionprofile', notary_profile='test-notary')
            config.write_text(json.dumps(valid))
            self.assertEqual(release.load_configuration(config)['identity'], IDENTITY)
            for invalid in [{**valid, 'password': 'never-store'}, {**valid, 'identity': '-'},
                            {**valid, 'profile': 'relative.profile'}, {**valid, 'notary_profile': ''}]:
                config.write_text(json.dumps(invalid))
                with self.assertRaises(ValueError):
                    release.load_configuration(config)

    def test_missing_developer_id_stops_before_reading_profile_or_building(self):
        with patch.object(release.wallet, 'capture', return_value=b'0 valid identities found'), \
                patch.object(release.wallet, 'decode_profile') as decode:
            with self.assertRaisesRegex(ValueError, 'Developer ID Application'):
                release.preflight(dict(identity=IDENTITY, profile=Path('/not/read'), notary_profile='test'))
            decode.assert_not_called()

    def test_notary_rejection_never_staples(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'submission.json'
            with patch.object(release.subprocess, 'run') as run:
                run.return_value.stdout = '{"id":"test-id","status":"Invalid"}'
                with self.assertRaisesRegex(ValueError, 'did not accept'):
                    release.submit(Path('/test.dmg'), dict(notary_profile='test'), report)
                self.assertEqual(run.call_count, 1)
                self.assertEqual(json.loads(report.read_text())['status'], 'Invalid')


if __name__ == '__main__':
    unittest.main()
