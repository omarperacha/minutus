"""Build and inspect the Apple Silicon app with local or Developer ID seals."""

import hashlib
import argparse
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess

from inspect_macos_release import ROOT, inspect

OUTPUT = ROOT / 'target/aarch64-apple-darwin/release/bundle'
APPLICATION = OUTPUT / 'macos/Minutus.app'


def run(command, log):
    print('Running: ' + ' '.join(map(str, command)), flush=True)
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True) as process:
        for line in process.stdout:
            log.write(line)
            log.flush()
            print(line, end='', flush=True)
        if process.wait():
            raise subprocess.CalledProcessError(process.returncode, command)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--distribution-config', type=Path)
    args = parser.parse_args()
    distribution = None
    if args.distribution_config:
        reviewed = json.loads((ROOT / 'scripts/qwen_accel_bundled.lock.json').read_text())
        payload = json.loads((ROOT / 'src-tauri/ollama-runtime/qwen-accel/payload.json').read_text())
        if payload['source_artifact_sha256'] != reviewed['artifact_sha256']:
            raise ValueError('Official signing requires the committed reviewed Qwen artifact; a local source rebuild needs separate qualification')
        from macos_distribution import load_configuration, preflight
        distribution = load_configuration(args.distribution_config)
        preflight(distribution)
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise ValueError('Apple Silicon macOS is required')
    pnpm = shutil.which('pnpm')
    if pnpm is None:
        raise ValueError('pnpm is unavailable')
    # Never overwrite a running bundle. No application launch or model download.
    running = subprocess.run(['pgrep', '-f', f'^{re.escape(str(APPLICATION))}/Contents/'],
                             capture_output=True, check=False)
    if running.returncode != 1:
        raise ValueError('Quit the release app before rebuilding (or check pgrep)')
    OUTPUT.mkdir(parents=True, exist_ok=True)
    with (OUTPUT / 'release-build.log').open('w') as log:
        run([pnpm, 'tauri', 'build', '--target', 'aarch64-apple-darwin',
             '--bundles', 'app', '--no-sign', '--ci', '--', '--locked'], log)
        unsigned = inspect(APPLICATION, verify_unsigned=True)
        (OUTPUT / 'unsigned-package.json').write_text(json.dumps(unsigned, indent=2) + '\n')
        # Existing signer owns XPC relocation and helper confinement. '-' is
        # explicitly its local packaging mode, without Apple credentials.
        if distribution:
            from macos_distribution import wallet, verify
            wallet.sign(APPLICATION, distribution['identity'], distribution['profile'])
            verify(APPLICATION, distribution)
        else:
            run(['sh', str(ROOT / 'scripts/sign-macos-bundle.sh'), str(APPLICATION), '-'], log)
        run(['sh', str(ROOT / 'scripts/verify-macos-bundle.sh'), str(APPLICATION)], log)
        packaged = inspect(APPLICATION)
        packaged['unsigned_report_sha256'] = hashlib.sha256(
            (OUTPUT / 'unsigned-package.json').read_bytes()).hexdigest()
        packaged['local_ad_hoc_seal_verified'] = distribution is None
        packaged['distribution_signed'] = distribution is not None
        (OUTPUT / 'package-inspection.json').write_text(json.dumps(packaged, indent=2) + '\n')
    print(f'Apple Silicon package prepared: {APPLICATION}')
    print('Developer ID signatures verified; notarization remains required.' if distribution
          else 'Local ad-hoc seal verified; wallet functionality needs no Apple developer credentials. Public distribution signing/notarization is a separate step.')


if __name__ == '__main__':
    try:
        main()
    except ValueError as error:
        print(f'Release preparation stopped: {error}', flush=True)
        raise SystemExit(1) from None
