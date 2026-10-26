"""Cache maintenance runs before app execution and retains Cargo's process ID."""

import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

PATH = Path(__file__).with_name("run-dev-app.py")
SPEC = importlib.util.spec_from_file_location("run_dev_app", PATH)
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class RunDevAppTests(unittest.TestCase):
    def test_maintenance_finishes_before_exec_and_arguments_are_preserved(self):
        events = []
        with (
            patch.object(launcher.sys, "argv", [str(PATH), "/target/debug/app", "--example"]),
            patch.object(launcher, "maintain_workspace", side_effect=lambda: events.append("clean")),
            patch.object(launcher, "sign_runtime", return_value=0),
            patch.object(launcher.os, "execv", side_effect=lambda *args: events.append(args)),
        ):
            launcher.main()
        self.assertEqual(events, ["clean", ("/target/debug/app", ["/target/debug/app", "--example"])])

    def test_busy_cache_defers_without_preventing_app_launch(self):
        with (
            patch.object(launcher.sys, "argv", [str(PATH), "/target/debug/app"]),
            patch.object(launcher, "maintain_workspace", side_effect=launcher.CacheBusyError("busy")),
            patch.object(launcher, "sign_runtime", return_value=0),
            patch.object(launcher.os, "execv") as execute,
        ):
            launcher.main()
        execute.assert_called_once_with("/target/debug/app", ["/target/debug/app"])

    def test_failed_maintenance_does_not_silently_claim_success(self):
        with (
            patch.object(launcher.sys, "argv", [str(PATH), "/target/debug/app"]),
            patch.object(launcher, "maintain_workspace", side_effect=OSError("failed")),
            patch.object(launcher, "sign_runtime", return_value=0),
            patch.object(launcher.os, "execv") as execute,
        ):
            self.assertEqual(launcher.main(), 1)
        execute.assert_not_called()


if __name__ == "__main__":
    unittest.main()
