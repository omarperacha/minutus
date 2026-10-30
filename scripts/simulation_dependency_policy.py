"""Exact source, feature and applicability limits for the reviewed local EVM graph."""
from datetime import date, datetime, timezone
import tomllib

BASE_REVISION = '5476069f1eb74a56bccb56fdefd4b4bb0e3cda42'
BASE_SOURCE = f'git+https://github.com/base/base?rev={BASE_REVISION}#{BASE_REVISION}'
BASE_FEATURES = ['std', 'portable', 'blst', 'c-kzg', 'memory_limit']
BASE_PACKAGES = {
    'base-common-evm', 'base-common-chains', 'base-common-consensus',
    'base-common-eip8130', 'base-common-flz', 'base-common-genesis',
    'base-common-l1-fees', 'base-common-precompiles',
    'base-execution-eip8130', 'base-precompile-macros', 'base-precompile-storage', 'base-metrics',
}
FORBIDDEN = {'wallet-core', 'filesystem-tools', 'permissions', 'app-core',
             'app-storage', 'reqwest', 'hyper', 'tokio', 'tauri',
             'x402-client', 'web-retrieval'}
REGISTRY = 'registry+https://github.com/rust-lang/crates.io-index'
# Re-review on this UTC date, including whether upstream verification libraries migrated.
APPLICABILITY_DEADLINE = date(2027, 1, 30)
REVIEWED_CHECKSUMS = {
    ('alloy-consensus', '2.5.0'): '727016fed834c2392d014dd697ea473f3e47bcec31ba31ccffa9c1d18ed904e5',
    ('alloy-eip7702', '0.6.3'): '2919c5a56a1007492da313e7a3b6d45ef5edc5d33416fdec63c0d7a2702a0d20',
    ('alloy-primitives', '1.7.3'): 'd5e9dbd49258ac3ab893a481d46be29b58be7f734dcac46cd80b6b13ee36566c',
    ('aurora-engine-modexp', '1.3.0'): '5188e264926edbd2e90d61bf8b33aa3471db8acdf427fa37946f9c82898fe502',
    ('crypto-bigint', '0.5.5'): '0dc92fb57ca44df6db8059111ab3af99a63d5d0f8375d9972e319a379c6bab76',
    ('crypto-bigint', '0.7.9'): 'd811ea23fdfbcce2e2d5132344d471abdb8f901ac496a0893a3df92e70525e52',
    ('ecdsa', '0.16.9'): 'ee27f32b5c5292967d2d4a9d7f1e0b0aed2c15daded5a60300e4abb9d8020bca',
    ('elliptic-curve', '0.13.8'): 'b5e6043086bf7973472e0c7dff2142ea0b680d30e18d9cc40f267efbf222bd47',
    ('k256', '0.13.4'): 'f6e3919bbaa2945715f0bb6d3934a173d1e9a59ac23767fbaaef277265a7411b',
    ('p256', '0.13.2'): 'c9863ad85fa8f4460f9c48cb909d38a0d689dba1f6f6988a5e3e0d31071bcd4b',
    ('primeorder', '0.13.6'): '353e1ca18966c16d9deb1c69278edbc5f194139612772bd9537af60ac231e1e6',
    ('revm', '42.0.1'): 'b7c7fcd426beab3b91300a6d0d5eb8d583c0fad0fba451126e19fd51443eca62',
    ('revm', '43.0.3'): '62b2fbb3bdce72fef4f156e2164bdfde44dc3aeebc5b4f31a413fba0c09c524c',
    ('paste', '1.0.15'): '57c0d7b74b563b49d38dae00a0c37d4d6de9b432382b2892f0574ddcae73fd0a',
    ('syn-solidity', '1.7.3'): 'eb6a2e3c7f7a3e4e83d1752cec5d1e357ced0cf96e85419b6a07f227db3def3a',
    ('ark-ff', '0.3.0'): '6b3235cc41ee7a12aaaf2c575a2ad7b46713a8a50bda2fc3b003a04845c05dd6',
    ('ark-ff', '0.4.2'): 'ec847af850f44ad29048935519032c33da8aa03340876d351dfab5660d2966ba',
    ('ark-ff', '0.5.0'): 'a177aba0ed1e0fbb62aa9f6d0502e9b46dad8c2eab04c14258a1212d2557ea70',
    ('revm-precompile', '42.0.1'): 'f2431091420999795df6a200b9f7933defe3d654710560a895e6bcabbd9954a5',
    ('revm-precompile', '43.0.3'): 'aa80b8cef09bead8c8b1afa7733b09fc1428f49158d8657c239268409d2a2bf7',
}
EXTRA_PASTE_PARENTS = {('alloy-primitives', '1.7.3'), ('syn-solidity', '1.7.3'),
                       ('ark-ff', '0.3.0'), ('ark-ff', '0.4.2'), ('ark-ff', '0.5.0')}
BASE_RESOLVED_FEATURES = {name: {'std'} for name in BASE_PACKAGES}
BASE_RESOLVED_FEATURES.update({
    'base-common-chains': {'default', 'std'},
    'base-common-consensus': {'evm', 'k256', 'std'},
    'base-common-evm': set(BASE_FEATURES) | {'base-precompile-storage'},
    'base-common-precompiles': set(BASE_FEATURES),
    'base-execution-eip8130': set(), 'base-precompile-macros': set(),
})
PRECOMPILES = {('revm-precompile', '42.0.1'), ('revm-precompile', '43.0.3')}
BASE_VERIFIER = ('base-execution-eip8130', '1.5.1')
# Direct parents deliberately include the existing Alloy callers outside the worker.
# This is public signature recovery/verification, not a claim that all calldata is public.
REVIEWED_PARENTS = {
    ('crypto-bigint', '0.5.5'): {('elliptic-curve', '0.13.8')},
    ('elliptic-curve', '0.13.8'): {('ecdsa', '0.16.9'), ('k256', '0.13.4'),
                                ('p256', '0.13.2'), ('primeorder', '0.13.6')},
    ('ecdsa', '0.16.9'): {('k256', '0.13.4'), ('p256', '0.13.2')},
    ('primeorder', '0.13.6'): {('p256', '0.13.2')},
    ('k256', '0.13.4'): PRECOMPILES | {BASE_VERIFIER, ('alloy-consensus', '2.5.0'),
                                                      ('alloy-eip7702', '0.6.3'),
                                                      ('alloy-primitives', '1.7.3')},
    ('p256', '0.13.2'): PRECOMPILES | {BASE_VERIFIER},
    ('aurora-engine-modexp', '1.3.0'): PRECOMPILES,
}
REVIEWED_WORKSPACE_CONSUMERS = {'app-core', 'chain-evm', 'evm-simulation',
                                'x402-client', 'zeta-desktop'}


def _reachable(start, nodes):
    pending, seen = [start], set()
    while pending:
        item = pending.pop()
        if item in seen:
            continue
        seen.add(item)
        pending.extend(dependency['pkg'] for dependency in nodes[item]['deps'])
    return seen


def _validate_applicability(metadata, root, packages, nodes, today):
    if today >= APPLICABILITY_DEADLINE:
        raise ValueError('Local simulation dependency applicability review expired')
    lock = tomllib.loads((root / 'Cargo.lock').read_text())['package']
    selected = {}
    for key, checksum in REVIEWED_CHECKSUMS.items():
        matches = [item for item in packages.values() if (item['name'], item['version']) == key]
        locked = [item for item in lock if (item['name'], item['version']) == key]
        if (len(matches) != 1 or len(locked) != 1 or matches[0].get('source') != REGISTRY
                or locked[0].get('source') != REGISTRY or locked[0].get('checksum') != checksum):
            raise ValueError(f'Reviewed dependency version/source/checksum changed or obsolete: {key}')
        selected[key] = matches[0]['id']
    for name, versions in [('crypto-bigint', {'0.5.5', '0.7.9'}),
                           ('aurora-engine-modexp', {'1.3.0'})]:
        if {item['version'] for item in packages.values() if item['name'] == name} != versions:
            raise ValueError(f'Reviewed {name} versions changed; applicability review required')
    aurora = packages[selected[('aurora-engine-modexp', '1.3.0')]]
    if aurora.get('license') != 'CC0-1.0':
        raise ValueError('Aurora licence changed; exact package licence review required')
    for key, allowed in REVIEWED_PARENTS.items():
        direct = {(packages[parent]['name'], packages[parent]['version'])
                  for parent, node in nodes.items()
                  if any(dependency['pkg'] == selected[key] for dependency in node['deps'])}
        if direct != allowed:
            raise ValueError(f'Reviewed public verification dependency path changed: {key}')
    old = selected[('crypto-bigint', '0.5.5')]
    ancestors = {packages[item]['name'] for item in metadata['workspace_members']
                 if old in _reachable(item, nodes)}
    if ancestors != REVIEWED_WORKSPACE_CONSUMERS:
        raise ValueError('Reviewed public verification workspace consumers changed')
    wallets = [item['id'] for item in packages.values() if item['name'] == 'wallet-core']
    if len(wallets) != 1:
        raise ValueError('Expected one wallet authority boundary for dependency separation')
    wallet_graph = _reachable(wallets[0], nodes)
    if old in wallet_graph or selected[('crypto-bigint', '0.7.9')] not in wallet_graph:
        raise ValueError('Wallet crypto-bigint separation or patched version changed')
    # A new native secp256k1 path needs its own licence and implementation review.
    for item in packages.values():
        if item['name'] in {'revm', 'revm-precompile', 'alloy-primitives'}:
            if 'secp256k1' in nodes[item['id']]['features']:
                raise ValueError('Unreviewed native secp256k1 feature acquired')


def validate_simulation_dependencies(metadata, root, today=None):
    today = today if today is not None else datetime.now(timezone.utc).date()
    manifest = tomllib.loads((root / 'Cargo.toml').read_text())
    dependencies = manifest['workspace']['dependencies']
    base = dependencies['base-evm']
    if (base.get('git') != 'https://github.com/base/base'
            or base.get('rev') != BASE_REVISION or base.get('version') != '=1.5.1'
            or base.get('package') != 'base-common-evm'
            or base.get('default-features') is not False
            or set(base.get('features', [])) != set(BASE_FEATURES)):
        raise ValueError('Base source/features changed; exact local-engine review required')
    consensus = dependencies.get('base-consensus', {})
    if (consensus.get('git') != 'https://github.com/base/base'
            or consensus.get('rev') != BASE_REVISION or consensus.get('version') != '=1.5.1'
            or consensus.get('package') != 'base-common-consensus'
            or consensus.get('default-features') is not False
            or consensus.get('features') != ['std']):
        raise ValueError('Base receipt codec source/features changed; exact review required')
    for name, version in [('revm', '=43.0.3'), ('revm-base', '=42.0.1')]:
        selected = dependencies[name]
        if (selected.get('version') != version or selected.get('default-features') is not False
                or set(selected.get('features', [])) != set(BASE_FEATURES)):
            raise ValueError('Revm version/features changed; exact engine review required')
    packages = {item['id']: item for item in metadata['packages']}
    nodes = {item['id']: item for item in metadata['resolve']['nodes']}
    git = [item for item in packages.values() if (item.get('source') or '').startswith('git+')]
    if len(git) != len(BASE_PACKAGES) or {item['name'] for item in git} != BASE_PACKAGES or any(
            item['source'] != BASE_SOURCE or item['version'] != '1.5.1' or item.get('license') != 'MIT'
            for item in git):
        raise ValueError('Git package/revision/licence graph changed; Base review required')
    engines = [item for item in packages.values() if item['name'] == 'revm']
    if len(engines) != 2 or {item['version'] for item in engines} != {'42.0.1', '43.0.3'}:
        raise ValueError('Resolved Revm versions differ from reviewed engines')
    precompiles = [item for item in packages.values() if item['name'] == 'revm-precompile']
    for item in git + engines + precompiles:
        features = set(nodes[item['id']]['features'])
        allowed = (BASE_RESOLVED_FEATURES[item['name']] if item in git else
                   set(BASE_FEATURES) - ({'memory_limit'} if item in precompiles else set()))
        if features != allowed:
            raise ValueError('Local engine acquired relaxed validation or unreviewed features')
    child = next(item for item in packages.values() if item['name'] == 'evm-simulation')
    reachable = _reachable(child['id'], nodes)
    forbidden = {packages[item]['name'] for item in reachable} & FORBIDDEN
    if forbidden:
        raise ValueError('EVM child acquired forbidden host/network dependencies: ' + ', '.join(sorted(forbidden)))
    _validate_applicability(metadata, root, packages, nodes, today)
