"""Source-build orchestration uses synthetic tools; no downloads or app launch."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class SetupReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scripts = self.root / 'scripts'
        scripts.mkdir()
        shutil.copyfile(Path(__file__).with_name('setup-release.sh'), scripts / 'setup-release.sh')
        (self.root / '.nvmrc').write_text('22.23.4\n')
        (self.root / 'package.json').write_text(json.dumps({'packageManager': 'pnpm@10.2.0'}))
        (scripts / 'bootstrap.py').write_text(
            'import os\nwith open(os.environ["BUILD_TEST_LOG"], "a") as log: log.write("bootstrap\\n")\n')
        tools = self.root / 'bin'
        tools.mkdir()
        self.log = self.root / 'steps.log'
        self.env = dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}',
                        NVM_DIR=str(self.root / 'no-nvm'),
                        BUILD_TEST_LOG=str(self.log))
        for name, body in {
            'uname': 'if [ "$1" = -s ]; then echo Darwin; else echo "${BUILD_TEST_ARCH:-arm64}"; fi',
            'node': 'echo "${BUILD_TEST_NODE:-v22.23.4}"',
            'cmake': 'echo "cmake version 3.24.0"',
            'xcrun': 'exit 0', 'rustup': 'exit 0', 'cargo': 'exit 0', 'rustc': 'exit 0',
            'pnpm': '''if [ "$1" = --version ]; then echo "${BUILD_TEST_PNPM:-10.2.0}"; exit; fi
echo "pnpm $*" >> "$BUILD_TEST_LOG"
if [ "$1" = "${BUILD_TEST_FAIL:-}" ]; then exit 7; fi''',
        }.items():
            tool = tools / name
            tool.write_text('#!/bin/bash\n' + body + '\n')
            tool.chmod(0o755)

    def run_script(self, *args):
        return subprocess.run(['/bin/bash', str(self.root / 'scripts/setup-release.sh'), *args],
                              cwd='/', env=self.env, text=True, capture_output=True, check=False)

    def test_build_steps_are_ordered_and_launch_is_separate(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), [
            'pnpm install --frozen-lockfile', 'bootstrap', 'pnpm check', 'pnpm release:build'])
        self.assertIn('open target/aarch64-apple-darwin/release/bundle/macos/Minutus.app', result.stdout)

    def test_failed_check_prevents_release_build(self):
        self.env['BUILD_TEST_FAIL'] = 'check'
        result = self.run_script()
        self.assertEqual(result.returncode, 7)
        self.assertNotIn('release:build', self.log.read_text())

    def test_nvm_selects_the_repository_pin_before_building(self):
        nvm_dir = self.root / 'nvm'
        nvm_dir.mkdir()
        (nvm_dir / 'nvm.sh').write_text(
            'nvm() { echo "nvm $*" >> "$BUILD_TEST_LOG"; }\n')
        self.env['NVM_DIR'] = str(nvm_dir)
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines()[:3], [
            'nvm install 22.23.4', 'nvm use 22.23.4', 'pnpm install --frozen-lockfile'])

    def test_wrong_toolchain_stops_before_dependency_installation(self):
        for variable, value in [('BUILD_TEST_NODE', 'v23.7.0'), ('BUILD_TEST_PNPM', '9.0.0')]:
            with self.subTest(variable=variable):
                self.env[variable] = value
                self.assertNotEqual(self.run_script().returncode, 0)
                self.assertFalse(self.log.exists())
                del self.env[variable]

    def test_intel_mac_stops_before_build(self):
        self.env['BUILD_TEST_ARCH'] = 'x86_64'
        self.assertNotEqual(self.run_script().returncode, 0)
        self.assertFalse(self.log.exists())

    def test_help_does_not_run_build_steps(self):
        self.assertEqual(self.run_script('--help').returncode, 0)
        self.assertFalse(self.log.exists())


if __name__ == '__main__':
    unittest.main()
