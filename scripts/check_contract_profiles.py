#!/usr/bin/env python3
"""Check the immutable reviewed contract artifact root; optionally reproduce solc output.

No runtime compiler is installed. --reproduce downloads only the fixed official
build-only compiler into an isolated temporary directory after verifying its hash.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
PROFILE = ROOT / 'crates/chain-evm/profiles/erc20-oz-5.7.0'
MANIFEST_SHA256 = '7f90b52883050e63b66d7215d0571f0d18cc3b7f2ab5d299074200af7568044b'
REVISION = '76df8832488cb0beac36a11b032a1240a39f05202b4c604d580e0485645feef7'
CORE_NAMES = ('schemaVersion', 'profileId', 'sourceRevision', 'sourceBundleSha256',
              'compilerSha256', 'inputSha256', 'runtimeKeccak256', 'abiSha256',
              'storageLayoutSha256')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def verify_profile(root=PROFILE):
    manifest_bytes = (root / 'manifest.json').read_bytes()
    if sha(manifest_bytes) != MANIFEST_SHA256:
        raise ValueError('Reviewed profile manifest changed')
    manifest = json.loads(manifest_bytes)
    core = {name: manifest[name] for name in CORE_NAMES}
    if sha(json.dumps(core, sort_keys=True, separators=(',', ':')).encode()) != REVISION:
        raise ValueError('Reviewed source/compiler/settings/artifact root changed')
    for name, expected in manifest['filesSha256'].items():
        path = root / name
        if path.is_symlink() or sha(path.read_bytes()) != expected:
            raise ValueError(f'Reviewed artifact changed: {name}')
    source_root = root / 'sources'
    actual = {path.relative_to(source_root).as_posix(): sha(path.read_bytes())
              for path in source_root.rglob('*.sol') if not path.is_symlink()}
    if actual != manifest['sourceFilesSha256']:
        raise ValueError('Reviewed source bundle changed')
    embedded = json.loads((root / 'input.json').read_bytes())['sources']
    if {name: item['content'] for name, item in embedded.items()} != {
            name: (source_root / name).read_text() for name in actual}:
        raise ValueError('Embedded compiler sources differ from reviewed source files')
    runtime = bytes.fromhex((root / 'runtime.hex').read_text().strip().removeprefix('0x'))
    if len(runtime) != manifest['runtimeBytes'] or sha(runtime) != manifest['runtimeBytesSha256']:
        raise ValueError('Complete runtime changed')
    output = json.loads((root / 'output.json').read_bytes())
    artifact = output['contracts']['MinutusQualifiedERC20.sol']['MinutusQualifiedERC20']
    evm = artifact['evm']
    if (evm['bytecode']['linkReferences'] or evm['deployedBytecode']['linkReferences']
            or evm['deployedBytecode']['immutableReferences']):
        raise ValueError('Unsupported linked or immutable code')
    selectors = evm['methodIdentifiers']
    if len(set(selectors.values())) != len(selectors):
        raise ValueError('Selector collision')
    return manifest


def reproduce(manifest, compiler, node):
    if sha(compiler.read_bytes()) != manifest['compilerSha256']:
        raise ValueError('Official build-only compiler checksum mismatch')
    subprocess.run([sys.executable, str(PROFILE / 'rebuild.py'), '--soljson',
                    str(compiler), '--node', node], check=True, timeout=100)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, help='Checksum-verified local official solc WASM JS')
    parser.add_argument('--reproduce', action='store_true', help='Download fixed official compiler and reproduce')
    parser.add_argument('--node', default='node', help='Pinned Node22.23.4 executable')
    args = parser.parse_args()
    manifest = verify_profile()
    if args.compiler:
        reproduce(manifest, args.compiler.resolve(), args.node)
    elif args.reproduce:
        with tempfile.TemporaryDirectory(prefix='minutus-profile-compiler-') as directory:
            compiler = Path(directory) / 'soljson.js'
            with urllib.request.urlopen(manifest['compiler']['downloadUrl'], timeout=30) as response:
                data = response.read(64 * 1024 * 1024 + 1)
            if len(data) > 64 * 1024 * 1024:
                raise ValueError('Compiler exceeds reviewed download bound')
            compiler.write_bytes(data)
            reproduce(manifest, compiler, args.node)
    print('Reviewed contract profile source, compiler settings and artifacts match immutable pins')


if __name__ == '__main__':
    main()
