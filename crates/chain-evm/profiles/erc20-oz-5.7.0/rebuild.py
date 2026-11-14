#!/usr/bin/env python3
"""Reproduce this fixed reviewed source profile with a checksum-verified local compiler.

This build-only tool does not fetch a compiler, read wallet data, sign, submit,
accept arbitrary contract sources or enable a runtime profile.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
COMPILER_VERSION = '0.8.38+commit.1835d247.Emscripten.clang'
COMPILER_SHA256 = '6c74ffba0ea4cf9521e78b16052a98208770bb7677616de08ced239f4312d390'
PROFILE_COMMITMENT_SHA256 = '76df8832488cb0beac36a11b032a1240a39f05202b4c604d580e0485645feef7'
RUNTIME_SHA256 = '64f6b8370f45d7d91648b5a2b885f19a8a39f9d0f973b287678470f0152b3a94'
DRIVER = r'''
const fs = require('node:fs');
if (process.version !== 'v22.23.4') throw new Error('Use pinned Node22.23.4');
const compiler = require(process.argv[1]);
const version = compiler.cwrap('solidity_version', 'string', [])();
if (version !== '0.8.38+commit.1835d247.Emscripten.clang') throw new Error('Compiler version changed');
const compile = compiler.cwrap('solidity_compile', 'string', ['string','number','number']);
const input = fs.readFileSync(0, 'utf8');
process.stdout.write(JSON.stringify(JSON.parse(compile(input, 0, 0))));
'''


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def generate(soljson, node):
    manifest = json.loads((ROOT / 'manifest.json').read_bytes())
    if sha(soljson.read_bytes()) != COMPILER_SHA256:
        raise ValueError('Compiler does not match the published/reviewed checksum')
    core_names = ['schemaVersion', 'profileId', 'sourceRevision', 'sourceBundleSha256',
                  'compilerSha256', 'inputSha256', 'runtimeKeccak256', 'abiSha256',
                  'storageLayoutSha256']
    core = {name: manifest[name] for name in core_names}
    if sha(json.dumps(core, sort_keys=True, separators=(',', ':')).encode()) != PROFILE_COMMITMENT_SHA256:
        raise ValueError('Profile manifest identity changed; independent review required')
    for name in ['compiler-record.json', 'OpenZeppelin-MIT.txt']:
        if sha((ROOT / name).read_bytes()) != manifest['filesSha256'][name]:
            raise ValueError(f'Pinned compiler record/notice changed: {name}')
    source_root = ROOT / 'sources'
    source_files = {p.relative_to(source_root).as_posix(): p for p in source_root.rglob('*.sol')}
    actual = {name: sha(path.read_bytes()) for name, path in source_files.items()}
    if actual != manifest['sourceFilesSha256']:
        raise ValueError('Source bundle changed; create and review a new profile revision')
    source_bundle = sha(json.dumps(actual, sort_keys=True, separators=(',', ':')).encode())
    if source_bundle != manifest['sourceBundleSha256']:
        raise ValueError('Source bundle identity changed')
    input_bytes = (ROOT / 'input.json').read_bytes()
    if sha(input_bytes) != manifest['inputSha256']:
        raise ValueError('Compiler input/settings changed; new profile review required')
    standard = json.loads(input_bytes)
    if {name: item['content'] for name, item in standard['sources'].items()} != {
            name: path.read_text() for name, path in source_files.items()}:
        raise ValueError('Embedded source input differs from the pinned source bundle')
    completed = subprocess.run([node, '-e', DRIVER, str(soljson.resolve())],
                               input=input_bytes, capture_output=True, check=True, timeout=90)
    output = json.loads(completed.stdout)
    if output.get('errors'):
        raise ValueError('Compiler reported diagnostics; inspect rather than ignoring warnings/errors')
    artifact = output['contracts']['MinutusQualifiedERC20.sol']['MinutusQualifiedERC20']
    evm = artifact['evm']
    if (evm['bytecode']['linkReferences'] or evm['deployedBytecode']['linkReferences']
            or evm['deployedBytecode']['immutableReferences']):
        raise ValueError('Unsupported link or immutable references')
    runtime = bytes.fromhex(evm['deployedBytecode']['object'])
    if sha(runtime) != RUNTIME_SHA256:
        raise ValueError('Compiled runtime changed; no metadata stripping or patching is supported')
    selectors = evm['methodIdentifiers']
    if len(set(selectors.values())) != len(selectors):
        raise ValueError('Selector collision in profile ABI')
    return {
        'output.json': canonical(output),
        'abi.json': canonical(artifact['abi']),
        'storage-layout.json': canonical(artifact['storageLayout']),
        'method-identifiers.json': canonical(selectors),
        'metadata.json': canonical(json.loads(artifact['metadata'])),
        'creation.hex': ('0x' + evm['bytecode']['object'] + '\n').encode(),
        'runtime.hex': ('0x' + evm['deployedBytecode']['object'] + '\n').encode(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--soljson', required=True, type=Path, help='Local official solc0.8.38 WASM JS file')
    parser.add_argument('--node', default='node', help='Pinned Node22.23.4 executable')
    parser.add_argument('--write', action='store_true', help='Regenerate deterministic output files')
    args = parser.parse_args()
    generated = generate(args.soljson, args.node)
    manifest = json.loads((ROOT / 'manifest.json').read_bytes())
    for name, data in generated.items():
        if sha(data) != manifest['filesSha256'][name]:
            raise ValueError(f'Reproduced output differs from reviewed manifest: {name}')
        if args.write:
            (ROOT / name).write_bytes(data)
        elif (ROOT / name).read_bytes() != data:
            raise ValueError(f'Checked-in output differs: {name}')
    print('Pinned compiler, six source files, ABI, runtime, layout and full output reproduce exactly')


if __name__ == '__main__':
    main()
