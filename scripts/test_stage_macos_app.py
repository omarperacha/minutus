"""Staging orchestration tests: no builds, signing, profiles or wallet access."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
from unittest.mock import call, patch

SPEC = importlib.util.spec_from_file_location(
    "stage_macos_app", Path(__file__).with_name("stage-macos-app.py")
)
staging = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(staging)


class StageMacosAppTests(unittest.TestCase):
    def setUp(self):
        # Never inspect or clean real build artifacts in orchestration tests.
        self.maintenance = patch.object(staging, "maintain").start()
        self.addCleanup(patch.stopall)

    def test_stage_release_and_hot_reload_use_distinct_identities(self):
        root = staging.REPOSITORY / "src-tauri"
        configs = [json.loads((root / name).read_text()) for name in
                   ("tauri.conf.json", "tauri.stage.conf.json", "tauri.dev.conf.json")]
        self.assertEqual(len({config["identifier"] for config in configs}), 3)
        self.assertEqual(configs[1]["productName"], "Minutus Staging")
        self.assertEqual(staging.APPLICATION.name, "Minutus Staging.app")

    def test_configuration_is_strict_and_resolves_the_profile(self):
        with tempfile.TemporaryDirectory(prefix="zeta-stage-test-") as directory:
            root = Path(directory)
            profile = root / "Minutus.provisionprofile"
            profile.write_bytes(b"profile metadata")
            configuration = root / ".zeta-stage.json"
            configuration.write_text(
                json.dumps({"identity": "a" * 40, "profile": str(profile)}),
                encoding="utf-8",
            )
            identity, actual_profile = staging.load_configuration(configuration)
            self.assertEqual(identity, "A" * 40)
            self.assertEqual(actual_profile, profile.resolve())

            configuration.write_text(
                json.dumps({"identity": "a" * 40, "profile": str(profile), "launch": True}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "only identity and profile"):
                staging.load_configuration(configuration)

            configuration.write_text(
                json.dumps({"identity": "a" * 40, "profile": "relative.profile"}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "must be absolute"):
                staging.load_configuration(configuration)

    def test_missing_or_invalid_configuration_fails_closed(self):
        with tempfile.TemporaryDirectory(prefix="zeta-stage-test-") as directory:
            root = Path(directory)
            missing = root / "missing.json"
            with self.assertRaisesRegex(ValueError, "Create .zeta-stage.json"):
                staging.load_configuration(missing)
            invalid = root / "invalid.json"
            invalid.write_text('{"identity":"not-a-hash","profile":"missing"}', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "40-character"):
                staging.load_configuration(invalid)

    def test_stage_builds_before_delegating_to_the_existing_signer(self):
        profile = Path("/tmp/Minutus.provisionprofile")
        application = Path("/tmp/Minutus.app")
        with (
            patch.object(staging, "bundle_process_is_running", return_value=False),
            patch.object(staging, "process_is_running", return_value=False),
            patch.object(staging.shutil, "which", return_value="/opt/pnpm"),
            patch.object(staging, "environment", return_value={"ZETA_X402_DEBUG_BASE_URL": "http://localhost:5001"}),
            patch.object(staging.subprocess, "run") as run,
        ):
            staging.stage("A" * 40, profile, application)
        self.assertEqual(
            run.call_args_list,
            [
                call(
                    ["/opt/pnpm", "tauri", "build", "--debug", "--config", "src-tauri/tauri.stage.conf.json", "--bundles", "app", "--", "--timings"],
                    cwd=staging.REPOSITORY,
                    env={"ZETA_X402_DEBUG_BASE_URL": "http://localhost:5001"},
                    check=True,
                ),
                call(
                    [
                        staging.sys.executable,
                        str(staging.REPOSITORY / "scripts/macos-wallet-signing.py"),
                        "sign",
                        str(application),
                        "A" * 40,
                        str(profile),
                    ],
                    cwd=staging.REPOSITORY,
                    check=True,
                ),
            ],
        )

    def test_default_stage_uses_ad_hoc_signing_without_reading_apple_configuration(self):
        with (
            patch.object(staging, "bundle_process_is_running", return_value=False),
            patch.object(staging, "process_is_running", return_value=False),
            patch.object(staging.shutil, "which", return_value="/opt/pnpm"),
            patch.object(staging, "environment", return_value={}),
            patch.object(staging, "load_configuration", side_effect=AssertionError("Apple configuration must not be read")),
            patch.object(staging.subprocess, "run") as run,
        ):
            staging.stage()
        commands = [entry.args[0] for entry in run.call_args_list]
        self.assertEqual(commands[1], ["sh", str(staging.REPOSITORY / "scripts/sign-macos-bundle.sh"), str(staging.APPLICATION), "-"])
        self.assertEqual(commands[2], ["sh", str(staging.REPOSITORY / "scripts/verify-macos-bundle.sh"), str(staging.APPLICATION)])
        self.assertFalse(any("macos-wallet-signing.py" in str(command) for command in commands))

    def test_running_staged_app_stops_before_build(self):
        with (
            patch.object(staging, "bundle_process_is_running", return_value=True),
            patch.object(staging, "process_is_running", return_value=False),
            patch.object(staging.subprocess, "run") as run,
        ):
            with self.assertRaisesRegex(ValueError, "Quit every running Minutus app"):
                staging.stage("A" * 40, Path("/tmp/profile"), Path("/tmp/Minutus.app"))
        run.assert_not_called()
        self.maintenance.assert_not_called()

    def test_failed_build_still_maintains_cache_without_signing(self):
        with (
            patch.object(staging, "bundle_process_is_running", return_value=False),
            patch.object(staging, "process_is_running", return_value=False),
            patch.object(staging.shutil, "which", return_value="/opt/pnpm"),
            patch.object(staging, "environment", return_value={}),
            patch.object(staging.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "build")) as run,
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                staging.stage("A" * 40, Path("/tmp/profile"), Path("/tmp/Minutus.app"))
        self.assertEqual(run.call_count, 1)
        self.maintenance.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
