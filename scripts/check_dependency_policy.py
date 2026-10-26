#!/usr/bin/env python3
"""Enforce the temporary paste exception before running all cargo-deny checks."""
from datetime import date, timezone, datetime
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REVIEW_DEADLINE = date(2027, 10, 13)


def validate_paste_exception(metadata, today):
    if today >= REVIEW_DEADLINE:
        raise ValueError('The temporary paste exception expired on 2027-10-13; review or remove it before release')
    packages = {item['id']: item for item in metadata['packages']}
    candidates = [item for item in packages.values() if item['name'] == 'paste']
    if not candidates:
        raise ValueError('paste is absent; remove the obsolete RUSTSEC-2024-0436 exception and this temporary guard')
    if len(candidates) != 1 or candidates[0]['version'] != '1.0.15':
        raise ValueError('The paste version changed; this temporary approval requires a new review')
    paste = candidates[0]
    library_kinds = [target['kind'] for target in paste['targets']
                     if 'lib' in target['kind'] or 'proc-macro' in target['kind']]
    if library_kinds != [['proc-macro']]:
        raise ValueError('paste is not the reviewed compile-time proc-macro')
    dependents = {
        (packages[node['id']]['name'], packages[node['id']]['version'])
        for node in metadata['resolve']['nodes']
        if any(dependency['pkg'] == paste['id'] for dependency in node['deps'])
    }
    if dependents != {('tokenizers', '0.22.2')}:
        raise ValueError('The paste dependency path changed; only tokenizers 0.22.2 is approved')


def main():
    try:
        today = datetime.now(timezone.utc).date()
        # Fail on expiry before any metadata fetching or dependency checks.
        if today >= REVIEW_DEADLINE:
            raise ValueError('The temporary paste exception expired on 2027-10-13; review or remove it before release')
        output = subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--all-features', '--format-version', '1'], cwd=ROOT, text=True)
        validate_paste_exception(json.loads(output), today)
        return subprocess.run(['cargo', 'deny', '--locked', 'check', '--hide-inclusion-graph'], cwd=ROOT, check=False).returncode
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f'Dependency policy failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
