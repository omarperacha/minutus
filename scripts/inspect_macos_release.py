"""Inspect release packaging independently of signing and notarization."""

import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess

ROOT = Path(__file__).resolve().parent.parent
MACH_MAGICS = {bytes.fromhex(value) for value in (
    'feedface', 'cefaedfe', 'feedfacf', 'cffaedfe', 'cafebabe', 'bebafeca',
    'cafebabf', 'bfbafeca',
)}
MODEL_SUFFIXES = {'.gguf', '.ggml', '.safetensors', '.onnx', '.pt', '.pth', '.ckpt'}
MODEL_MAGICS = {b'GGUF', b'ggml', b'ggmf', b'ggjt'}
REQUIRED_ARM64 = (
    'MacOS/zeta-desktop', 'MacOS/zeta-tokenizer', 'MacOS/zeta-whisper-helper',
    'MacOS/zeta-computation-broker',
    'Resources/ollama-runtime/ollama', 'Resources/ollama-runtime/llama-server',
    'Resources/ollama-runtime/qwen-accel/ollama',
    'Resources/ollama-runtime/qwen-accel/llama-server',
)


def capture(*command):
    return subprocess.check_output(command, text=True).strip()


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def safe_files(root):
    """Reject external links, special files and recognizable weight artifacts."""
    root = root.resolve()
    files = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink() and (not path.resolve().is_relative_to(root) or not path.is_file()):
            raise ValueError(f'Unsafe bundle link: {path.relative_to(root)}')
        if path.is_dir():
            if path.name.lower() in {'models', 'blobs', 'weights'}:
                raise ValueError(f'Model storage directory in bundle: {path.relative_to(root)}')
            continue
        if not path.is_file():
            raise ValueError(f'Unsupported bundle entry: {path.relative_to(root)}')
        with path.open('rb') as source:
            magic = source.read(4)
        if path.suffix.lower() in MODEL_SUFFIXES or magic in MODEL_MAGICS:
            raise ValueError(f'Model weights in bundle: {path.relative_to(root)}')
        files.append((path, magic))
    return files


def verify_manifest(directory, manifest):
    """Verify exact runtime contents against a repository-owned manifest."""
    directory = directory.resolve()
    expected = {}
    for line in manifest.read_text().splitlines():
        checksum, relative = line.split('  ', 1)
        relative = relative.removeprefix('./')
        path = Path(relative)
        if path.is_absolute() or '..' in path.parts or relative in expected:
            raise ValueError('Invalid runtime manifest path')
        source = directory / path
        if not source.is_file() or len(checksum) != 64 or digest(source) != checksum:
            raise ValueError(f'Runtime digest mismatch: {relative}')
        expected[relative] = checksum
    actual = {str(path.relative_to(directory)) for path, _ in safe_files(directory)
              if path != directory / 'payload.sha256'}
    if actual != expected.keys():
        raise ValueError('Runtime file set differs from the pinned manifest')
    if (directory / 'payload.sha256').read_bytes() != manifest.read_bytes():
        raise ValueError('Bundled runtime manifest differs from repository')


def inspect(application, verify_unsigned=False):
    contents = application.resolve() / 'Contents'
    files = safe_files(contents)
    info = plistlib.loads((contents / 'Info.plist').read_bytes())
    package = json.loads((ROOT / 'package.json').read_text())
    config = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_text())
    for key, expected in {
        'CFBundleIdentifier': config['identifier'],
        'CFBundleName': config['productName'],
        'CFBundleShortVersionString': package['version'],
        'CFBundleVersion': package['version'],
        'NSHumanReadableCopyright': config['bundle']['copyright'],
        'LSMinimumSystemVersion': '13.3',
    }.items():
        if info.get(key) != expected:
            raise ValueError(f'Incorrect application metadata: {key}')
    if digest(contents / 'Resources/_up_/LICENSE') != digest(ROOT / 'LICENSE'):
        raise ValueError('Bundled application license differs from repository')
    if verify_unsigned:
        verify_manifest(contents / 'Resources/ollama-runtime',
                        ROOT / 'src-tauri/ollama-runtime/payload.sha256')
        verify_manifest(contents / 'Resources/computation-service',
                        ROOT / 'src-tauri/computation-service/payload.sha256')
        for name in ('zeta-tokenizer', 'zeta-whisper-helper',
                     'zeta-computation-broker', 'zeta-computation-runner'):
            if digest(contents / 'MacOS' / name) != digest(
                    ROOT / 'src-tauri/binaries' / f'{name}-aarch64-apple-darwin'):
                raise ValueError(f'Bundled helper differs from staged input: {name}')
    native = []
    for path, magic in files:
        if magic not in MACH_MAGICS or path.is_symlink():
            continue
        architectures = capture('lipo', '-archs', str(path)).split()
        dependencies = []
        if 'arm64' in architectures:
            lines = capture('otool', '-arch', 'arm64', '-L', str(path)).splitlines()[1:]
            # otool includes a dylib's own install name; it is not a dependency.
            if path.suffix in {'.dylib', '.so'}:
                lines = lines[1:]
            for line in lines:
                dependency = line.strip().split(' (', 1)[0]
                if dependency.startswith(('/usr/lib/', '/System/Library/')):
                    continue
                if dependency.startswith(('@loader_path/', '@rpath/')):
                    # The shipped optional MLX libraries resolve adjacent files.
                    # Check the actual loader search path, not just file presence.
                    if dependency.startswith('@rpath/'):
                        commands = capture('otool', '-arch', 'arm64', '-l', str(path))
                        rpaths = [block.split('path ', 1)[1].split(' (offset', 1)[0]
                                  for block in commands.split('cmd LC_RPATH')[1:]]
                        if '@loader_path' not in rpaths:
                            raise ValueError(f'No adjacent loader rpath: {dependency}')
                    target = path.parent / dependency.split('/', 1)[1]
                    if not target.is_file() or not target.resolve().is_relative_to(contents):
                        raise ValueError(f'Missing bundled library: {dependency}')
                    if 'arm64' not in capture('lipo', '-archs', str(target)).split():
                        raise ValueError(f'Library lacks arm64: {dependency}')
                else:
                    raise ValueError(f'Nonportable native dependency: {dependency}')
                dependencies.append(dependency)
        native.append(dict(path=str(path.relative_to(contents)),
                           architectures=architectures, bundled_dependencies=dependencies))
    by_path = {entry['path']: entry for entry in native}
    required = list(REQUIRED_ARM64)
    prefix = ('Resources/computation-service' if verify_unsigned else 'XPCServices')
    required.extend(f'{prefix}/com.flowzed.zeta.computation-service.xpc/Contents/MacOS/{name}'
                    for name in ('zeta-computation-service', 'zeta-computation-runner'))
    for name in required:
        if 'arm64' not in by_path.get(name, {}).get('architectures', []):
            raise ValueError(f'Required executable lacks arm64: {name}')
    if by_path['MacOS/zeta-desktop']['architectures'] != ['arm64']:
        raise ValueError('Main application must be arm64-only')
    if not verify_unsigned and (contents / 'MacOS/zeta-computation-runner').exists():
        raise ValueError('Raw computation runner remains beside the main app')
    return dict(schema='minutus-macos-package-v1', version=package['version'],
                bundle_identifier=info['CFBundleIdentifier'],
                source_commit=capture('git', '-C', str(ROOT), 'rev-parse', 'HEAD'),
                working_tree=capture('git', '-C', str(ROOT), 'status', '--short'),
                target='aarch64-apple-darwin', model_weights_included=False,
                unsigned_payload_digests_verified=verify_unsigned,
                distribution_signed=False, notarized=False, native_files=native,
                file_count=len(files), bytes=sum(path.stat().st_size for path, _ in files),
                files={str(path.relative_to(contents)): digest(path) for path, _ in files})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('application', type=Path)
    parser.add_argument('--verify-unsigned', action='store_true')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    report = inspect(args.application, args.verify_unsigned)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Verified {report['file_count']} files; {len(report['native_files'])} native files; no model weights.")


if __name__ == '__main__':
    main()
