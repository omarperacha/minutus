#!/usr/bin/env python3
"""Pin and launch an experimental Qwen artifact through Minutus's debug-only provider route."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
LOCK = Path(__file__).with_suffix('.lock.json')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inspect(artifact):
    artifact = artifact.resolve()
    manifest = artifact / 'artifact.json'
    if manifest.is_symlink() or not manifest.is_file():
        raise ValueError('expected a regular artifact manifest')
    data = json.loads(manifest.read_text())
    if (data.get('schema') != 'qwen-metal-artifact-v1' or data.get('platform') != 'darwin-arm64'
            or data.get('ollama_version') != '0.32.14' or data.get('status') != 'experimental'):
        raise ValueError('unsupported experimental artifact')
    files = data.get('files', {})
    if not {'ollama', 'llama-server', 'serve.py', 'upstream.lock.json'} <= files.keys():
        raise ValueError('incomplete artifact')
    actual = {str(p.relative_to(artifact)) for p in artifact.rglob('*') if p.is_file() or p.is_symlink()}
    if actual != set(files) | {'artifact.json'}:
        raise ValueError('artifact contains missing or undeclared files')
    for name, expected in files.items():
        path = artifact / name
        if (Path(name).is_absolute() or '..' in Path(name).parts
                or not path.resolve().is_relative_to(artifact) or path.is_symlink()
                or not path.is_file() or digest(path) != expected):
            raise ValueError(f'artifact integrity failure: {name}')
    return dict(schema='zeta-experimental-qwen-pin-v1', artifact_sha256=digest(manifest),
        platform=data['platform'], ollama_version=data['ollama_version'],
        patch_sha256=data['patch_sha256'], files=files,
        production_promoted=False, profile='combined')


def verify_pin(artifact, lock=LOCK):
    observed = inspect(artifact)
    if observed != json.loads(lock.read_text()):
        raise ValueError('artifact differs from the reviewed development pin; use pin explicitly after review')
    return observed


def model_store(models, artifact):
    bundle = json.loads((artifact/'upstream.lock.json').read_text())['model']
    model = models/'blobs'/('sha256-'+bundle['gguf_sha256'])
    if model.is_symlink() or not model.is_file():
        raise ValueError('Minutus admission requires a regular GGUF blob; final-component symlinks are rejected')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['pin', 'verify', 'run'])
    parser.add_argument('--artifact', type=Path, required=True)
    parser.add_argument('--models', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--port', type=int, default=19485)
    parser.add_argument('--command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    artifact = args.artifact.resolve()
    try:
        if args.action == 'pin':
            LOCK.write_text(json.dumps(inspect(artifact), indent=2)+'\n')
            print(f'Pinned experimental artifact in {LOCK}; production bundle unchanged.')
            return 0
        verify_pin(artifact)
        if args.action == 'verify':
            print('Experimental artifact matches the Minutus development pin.')
            return 0
        if not args.models or not args.output or not 1024 <= args.port <= 65535:
            parser.error('run requires --models, a new --output directory, and a port in 1024–65535')
        models = args.models.resolve()
        model_store(models, artifact)
        from qwen_accel_runtime import prepare_loader, stage
        output = args.output.resolve()
        if output.exists():
            raise ValueError('run requires a new output directory')
        bundled = ROOT/'src-tauri/ollama-runtime'
        identity = digest(artifact/'artifact.json')+'-'+digest(bundled/'payload.sha256')
        cache = ROOT/'target/qwen-accel-runtime'/identity
        artifact = stage(artifact, bundled, cache, digest, inspect)
        inspect(artifact)
        prepare_loader(artifact)
        output.mkdir(parents=True, exist_ok=False)
        env = dict(os.environ, ZETA_DEV_OLLAMA_ADDRESS=f'127.0.0.1:{args.port}',
                   ZETA_DEV_OLLAMA_MODELS=str(models), ZETA_DEV_EXPERIMENTAL_MLX='1')
        command = args.command or [sys.executable, str(ROOT/'scripts/tauri-dev.py')]
        launch = [sys.executable, str(artifact/'serve.py'), '--artifact', str(artifact),
                  '--models', str(models), '--output', str(output/'server'),
                  '--port', str(args.port), '--profile', 'combined', '--command', *command]
        # The pinned launcher owns and terminates its provider/worker process groups.
        os.chdir(ROOT)
        os.execve(sys.executable, launch, env)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    raise SystemExit(main())
