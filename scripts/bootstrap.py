#!/usr/bin/env python3
"""Build Minutus's native dependencies from authenticated public source inputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / '.cache/minutus-source'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fetch(url, path, expected):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        partial = path.with_suffix(path.suffix + '.part')
        subprocess.run(['curl', '--fail', '--location', '--retry', '3', '--output', str(partial), url], check=True)
        if digest(partial) != expected:
            raise ValueError('download digest mismatch: ' + path.name)
        partial.replace(path)
    if not path.is_file() or path.is_symlink() or digest(path) != expected:
        raise ValueError('cached archive digest mismatch: ' + path.name)
    return path


def extract(archive, destination):
    # Reject links and special members before writing any source files.
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        roots = set()
        for member in members:
            parts = PurePosixPath(member.name).parts
            if not parts or member.name.startswith('/') or '..' in parts or not (member.isfile() or member.isdir()):
                raise ValueError('unsafe source archive member')
            roots.add(parts[0])
        if len(roots) != 1:
            raise ValueError('source archive must contain exactly one root')
        destination.mkdir(parents=True, exist_ok=False)
        bundle.extractall(destination, filter='data')
    return destination / roots.pop()


def run(*command, env=None):
    subprocess.run(list(map(str, command)), cwd=ROOT, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jobs', type=int, default=4)
    parser.add_argument('--runtime-only', action='store_true')
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ('Darwin', 'arm64') or not 1 <= args.jobs <= 16:
        parser.error('requires Apple Silicon macOS and 1–16 jobs')
    lock = json.loads((ROOT / 'scripts/public-runtime.lock.json').read_text())
    qwen = lock['qwen']
    archive = fetch(qwen['archive_url'], CACHE / 'qwen-source.tar.gz', qwen['archive_sha256'])
    artifact = CACHE / 'qwen-artifact'
    provenance = CACHE / 'source-build.json'
    if artifact.exists():
        if not provenance.is_file():
            raise ValueError('existing source artifact has no provenance; use a new checkout or remove the incomplete build')
        record = json.loads(provenance.read_text())
        if record['source_lock_sha256'] != digest(ROOT / 'scripts/public-runtime.lock.json'):
            raise ValueError('cached build belongs to a different source pin')
        from qwen_accel_dev import verify_pin
        verify_pin(artifact, CACHE / 'qwen-built.lock.json')
    else:
        attempt = Path(tempfile.mkdtemp(prefix='qwen-build-', dir=CACHE))
        source = extract(archive, attempt / 'source')
        upstream = json.loads((source / 'upstream.lock.json').read_text())
        if upstream['patch']['sha256'] != qwen['patch_sha256']:
            raise ValueError('public Qwen source changed its reviewed patch')
        if digest(source / upstream['patch']['file']) != qwen['patch_sha256']:
            raise ValueError('public Qwen patch digest mismatch')
        run(sys.executable, source / 'accel.py', 'build', '--work', attempt / 'work',
            '--cache', CACHE / 'downloads', '--output', artifact, '--jobs', args.jobs)
        from qwen_accel_dev import inspect
        built_pin = inspect(artifact)
        (CACHE / 'qwen-built.lock.json').write_text(json.dumps(built_pin, indent=2) + '\n')
        provenance.write_text(json.dumps(dict(
            source_lock_sha256=digest(ROOT / 'scripts/public-runtime.lock.json'),
            qwen_commit=qwen['commit'], artifact_sha256=built_pin['artifact_sha256'],
            qualification='local-source-build; not the reviewed official binary'), indent=2) + '\n')
    env = dict(os.environ, QWEN_ACCEL_ARTIFACT=str(artifact),
               QWEN_ACCEL_PIN=str(CACHE / 'qwen-built.lock.json'))
    run('sh', ROOT / 'scripts/fetch-ollama-runtime.sh', env=env)
    if not args.runtime_only:
        vendor = ROOT / 'vendor/llama.cpp'
        if not vendor.exists():
            vendor.parent.mkdir(parents=True, exist_ok=True)
            run('git', 'init', vendor)
            run('git', '-C', vendor, 'remote', 'add', 'origin', lock['tokenizer']['repository'])
            run('git', '-C', vendor, 'fetch', '--depth', '1', 'origin', lock['tokenizer']['commit'])
            run('git', '-C', vendor, 'checkout', '--detach', 'FETCH_HEAD')
        run('sh', ROOT / 'scripts/build-llama-sidecar.sh', args.jobs)
        speech = json.loads((ROOT / 'scripts/asr/models.lock.json').read_text())['runtime']
        fetch('https://api.github.com/repos/ggml-org/whisper.cpp/tarball/' + speech['commit'],
              ROOT / '.cache/asr-001/downloads/whisper-source.tar.gz', speech['source_archive_sha256'])
        run(sys.executable, ROOT / 'scripts/asr/stage_helper.py', '--target', 'aarch64-apple-darwin', '--jobs', args.jobs)
        run('sh', ROOT / 'scripts/build-computation-runner.sh')
    print('Source-built native payload prepared. No model weights were downloaded.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'Source bootstrap stopped: {error}') from None
