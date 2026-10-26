#!/usr/bin/env python3
"""Stage the reviewed Qwen acceleration payload beside the original bundled runtime."""
import argparse
import json
from pathlib import Path
import shutil
import tempfile

from qwen_accel_dev import digest, inspect, verify_pin
from qwen_accel_runtime import stage

ROOT = Path(__file__).resolve().parents[1]
PIN = Path(__file__).with_name('qwen_accel_bundled.lock.json')


def prepare(artifact, bundled, pin=PIN):
    artifact, bundled = artifact.resolve(), bundled.resolve()
    identity = verify_pin(artifact, pin)
    destination = bundled/'qwen-accel'
    # Do not turn unrelated local runtime tampering into a newly trusted manifest.
    # README and runtime.lock are reviewed source metadata, not executable inputs.
    expected_stock = {}
    for line in (bundled/'payload.sha256').read_text().splitlines():
        checksum, relative = line.split(maxsplit=1)
        relative = relative.removeprefix('./')
        path = Path(relative)
        if path.is_absolute() or '..' in path.parts or relative in expected_stock:
            raise ValueError('invalid existing bundled runtime manifest')
        expected_stock[relative] = checksum
        if relative.startswith('qwen-accel/') or relative in ('README.md', 'runtime.lock'):
            continue
        source = bundled/relative
        if not source.resolve().is_relative_to(bundled) or not source.is_file() or digest(source) != checksum:
            raise ValueError(f'original bundled runtime integrity failure: {relative}')
    actual_stock = {str(p.relative_to(bundled)) for p in bundled.rglob('*')
                    if (p.is_file() or p.is_symlink()) and p != bundled/'payload.sha256'
                    and not p.relative_to(bundled).parts[0] == 'qwen-accel'}
    if actual_stock != {name for name in expected_stock if not name.startswith('qwen-accel/')}:
        raise ValueError('original bundled runtime file set differs from its manifest')
    with tempfile.TemporaryDirectory(prefix='qwen-bundle-') as temporary:
        # Reuse the reviewed composition verifier for the identical provider and
        # bundled MLX libraries. Preserve those libraries for explicitly enabled
        # development uses, without enabling MLX in ordinary modes.
        composed = stage(artifact, bundled, Path(temporary)/'composition', digest, inspect)
        payload = Path(temporary)/'payload'
        payload.mkdir()
        selected = json.loads((composed/'artifact.json').read_text())['files']
        for name in selected:
            if name not in ('ollama', 'llama-server', 'upstream.lock.json') and not name.startswith(
                    ('licenses/', 'mlx_metal_v3/', 'mlx_metal_v4/')):
                continue
            target = payload/name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(composed/name, target)
        (payload/'NOTICE').write_text(
            'Retained Qwen Metal acceleration research. Copyright 2026 project contributors.\n'
            'Original project adaptations: Apache-2.0. Upstream ggml and Ollama portions: MIT.\n'
            'See licenses/ for full permission notices.\n')
        record = dict(schema='zeta-bundled-qwen-acceleration-v1',
                      source_artifact_sha256=identity['artifact_sha256'],
                      patch_sha256=identity['patch_sha256'], profile='combined',
                      hardware='Apple M1 Pro / 32 GiB / 10 CPU cores',
                      files={str(p.relative_to(payload)): digest(p)
                             for p in sorted(payload.rglob('*')) if p.is_file()})
        (payload/'payload.json').write_text(json.dumps(record, indent=2)+'\n')
        if destination.exists():
            expected = {str(p.relative_to(payload)): digest(p)
                        for p in payload.rglob('*') if p.is_file()}
            observed = {str(p.relative_to(destination)): digest(p)
                        for p in destination.rglob('*') if p.is_file() and not p.is_symlink()}
            if destination.is_symlink() or any(p.is_symlink() for p in destination.rglob('*')) or observed != expected:
                raise ValueError('existing bundled acceleration differs from the reviewed inputs')
        else:
            shutil.copytree(payload, destination)
    # Native bytes were verified above. The surrounding manifest also includes
    # reviewed source-controlled README/runtime metadata and the unchanged stock path.
    files = sorted(p for p in bundled.rglob('*') if p.is_file() and p != bundled/'payload.sha256')
    (bundled/'payload.sha256').write_text(''.join(
        f'{digest(p)}  ./{p.relative_to(bundled)}\n' for p in files))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifact', type=Path, required=True)
    parser.add_argument('--pin', type=Path, default=PIN, help='explicit local source-build pin or committed reviewed release pin')
    parser.add_argument('--bundled', type=Path, default=ROOT/'src-tauri/ollama-runtime')
    parser.add_argument('--verify-only', action='store_true', help='check the input pin before replacing upstream runtime files')
    args = parser.parse_args()
    if args.verify_only:
        verify_pin(args.artifact.resolve(), args.pin)
        print('Acceleration input matches the standard bundled pin.')
        return
    record = prepare(args.artifact.resolve(), args.bundled.resolve(), args.pin)
    print(json.dumps({k: v for k, v in record.items() if k != 'files'}, indent=2))


if __name__ == '__main__':
    main()
