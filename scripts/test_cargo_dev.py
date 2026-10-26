"""Tauri's runner must preserve Cargo's process identity across restarts."""

import importlib.machinery
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

PATH = Path(__file__).with_name("cargo-dev")
LOADER = importlib.machinery.SourceFileLoader("cargo_dev", str(PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
runner = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(runner)


class CargoDevTests(unittest.TestCase):
    def run_mock(self, arguments):
        with (
            patch.object(runner.sys, "argv", [str(PATH), *arguments]),
            patch.object(runner.os, "execvp") as launch,
            patch.object(runner, "maintain_workspace") as maintain,
        ):
            runner.main()
        return launch, maintain

    def test_run_installs_post_build_launcher_and_preserves_app_arguments(self):
        arguments = ["run", "--features", "tauri/devtools", "--timings", "--", "--config", "app.json"]
        launch, maintain = self.run_mock(arguments)
        program, actual = launch.call_args.args
        self.assertEqual(program, "cargo")
        self.assertEqual(actual[:2], ["cargo", "--config"])
        self.assertIn("run-dev-app.py", actual[2])
        self.assertEqual(actual[3:], arguments)
        maintain.assert_called_once_with()

    def test_queries_and_custom_directories_do_not_clean_the_wrong_cache(self):
        for arguments in (["--version"], ["run", "--target-dir=/tmp/custom"], ["build"]):
            launch, maintain = self.run_mock(arguments)
            launch.assert_called_once_with("cargo", ["cargo", *arguments])
            maintain.assert_not_called()

    def test_termination_reaches_the_cargo_child(self):
        with tempfile.TemporaryDirectory(prefix="zeta-cargo-runner-test-") as temporary:
            directory = Path(temporary)
            marker = directory / "stopped"
            cargo = directory / "cargo"
            cargo.write_text(
                f"#!{sys.executable}\n"
                "import signal, pathlib, sys\n"
                f"def stop(*args):\n pathlib.Path({str(marker)!r}).write_text('stopped')\n sys.exit(143)\n"
                "signal.signal(signal.SIGTERM, stop)\n"
                "import os\n"
                "print(os.getpid(), flush=True)\n"
                "signal.pause()\n"
            )
            cargo.chmod(0o755)
            environment = {**os.environ, "PATH": f"{directory}:{os.environ['PATH']}"}
            with subprocess.Popen([sys.executable, str(PATH), "--version"], env=environment, stdout=subprocess.PIPE, text=True) as process:
                try:
                    self.assertEqual(int(process.stdout.readline().strip()), process.pid)
                    process.send_signal(signal.SIGTERM)
                    self.assertEqual(process.wait(timeout=5), 143)
                    self.assertEqual(marker.read_text(), "stopped")
                finally:
                    if process.poll() is None:
                        process.kill()


if __name__ == "__main__":
    unittest.main()
