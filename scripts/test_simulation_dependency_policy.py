"""Mutation tests for the bounded simulation dependency review, without registry access."""
from copy import deepcopy
from datetime import date
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from simulation_dependency_policy import (
    APPLICABILITY_DEADLINE, BASE_FEATURES, BASE_PACKAGES, BASE_RESOLVED_FEATURES, BASE_REVISION, BASE_SOURCE,
    REGISTRY, REVIEWED_CHECKSUMS, REVIEWED_PARENTS, validate_simulation_dependencies,
)


def package_id(name, version):
    return f'{name}@{version}'


class SimulationDependencyPolicyTests(unittest.TestCase):
    def setUp(self):
        self.directory = TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = {
            'base-consensus': {'package': 'base-common-consensus', 'version': '=1.5.1',
                               'git': 'https://github.com/base/base', 'rev': BASE_REVISION,
                               'default-features': False, 'features': ['std']},
            'base-evm': {'package': 'base-common-evm', 'version': '=1.5.1',
                         'git': 'https://github.com/base/base', 'rev': BASE_REVISION,
                         'default-features': False, 'features': BASE_FEATURES.copy()},
            'revm': {'version': '=43.0.3', 'default-features': False, 'features': BASE_FEATURES.copy()},
            'revm-base': {'package': 'revm', 'version': '=42.0.1',
                          'default-features': False, 'features': BASE_FEATURES.copy()},
        }
        self.metadata = {'packages': [], 'workspace_members': [], 'resolve': {'nodes': []}}
        self.lock = []
        for (name, version), checksum in REVIEWED_CHECKSUMS.items():
            self.add_package(name, version, REGISTRY, 'CC0-1.0' if name == 'aurora-engine-modexp' else 'MIT')
            self.lock.append({'name': name, 'version': version, 'source': REGISTRY, 'checksum': checksum})
        for name in sorted(BASE_PACKAGES):
            self.add_package(name, '1.5.1', BASE_SOURCE)
            self.node(name, '1.5.1')['features'] = sorted(BASE_RESOLVED_FEATURES[name])
        for version in ['42.0.1', '43.0.3']:
            self.node('revm', version)['features'] = BASE_FEATURES.copy()
            self.node('revm-precompile', version)['features'] = ['std', 'portable', 'blst', 'c-kzg']
            self.edge(('revm', version), ('revm-precompile', version))
        for name in ['evm-simulation', 'chain-evm', 'x402-client', 'app-core', 'zeta-desktop', 'wallet-core']:
            self.add_package(name, '0.1.0', None)
            self.metadata['workspace_members'].append(package_id(name, '0.1.0'))
        for child, parents in REVIEWED_PARENTS.items():
            for parent in parents:
                self.edge(parent, child)
        self.edge(('evm-simulation', '0.1.0'), ('revm', '43.0.3'))
        self.edge(('evm-simulation', '0.1.0'), ('base-common-evm', '1.5.1'))
        self.edge(('base-common-evm', '1.5.1'), ('revm', '42.0.1'))
        self.edge(('base-common-evm', '1.5.1'), ('base-execution-eip8130', '1.5.1'))
        self.edge(('chain-evm', '0.1.0'), ('alloy-consensus', '2.5.0'))
        self.edge(('x402-client', '0.1.0'), ('chain-evm', '0.1.0'))
        self.edge(('app-core', '0.1.0'), ('x402-client', '0.1.0'))
        self.edge(('zeta-desktop', '0.1.0'), ('app-core', '0.1.0'))
        self.edge(('wallet-core', '0.1.0'), ('crypto-bigint', '0.7.9'))

    def add_package(self, name, version, source, license='MIT'):
        identifier = package_id(name, version)
        self.metadata['packages'].append({'id': identifier, 'name': name, 'version': version,
                                          'source': source, 'license': license})
        self.metadata['resolve']['nodes'].append({'id': identifier, 'deps': [], 'features': []})
        return identifier

    def package(self, name, version):
        return next(item for item in self.metadata['packages'] if item['id'] == package_id(name, version))

    def node(self, name, version):
        return next(item for item in self.metadata['resolve']['nodes'] if item['id'] == package_id(name, version))

    def edge(self, parent, child):
        self.node(*parent)['deps'].append({'pkg': package_id(*child)})

    def validate(self, today=date(2026, 10, 30)):
        lines = ['[workspace.dependencies]']
        for name, values in self.manifest.items():
            encoded = ', '.join(f'{key} = {json.dumps(value)}' for key, value in values.items())
            lines.append(f'{name} = {{ {encoded} }}')
        (self.root / 'Cargo.toml').write_text('\n'.join(lines))
        lines = []
        for item in self.lock:
            lines.append('[[package]]')
            lines.extend(f'{key} = {json.dumps(value)}' for key, value in item.items())
        (self.root / 'Cargo.lock').write_text('\n'.join(lines))
        validate_simulation_dependencies(self.metadata, self.root, today)

    def test_reviewed_graph_and_last_valid_day_pass(self):
        self.validate()
        self.validate(date(2027, 1, 29))

    def test_expiry_is_inclusive_and_cannot_silently_extend(self):
        for today in [APPLICABILITY_DEADLINE, date(2028, 1, 1)]:
            with self.subTest(today=today), self.assertRaisesRegex(ValueError, 'expired'):
                self.validate(today)

    def test_manifest_pins_and_features_cannot_drift(self):
        original = deepcopy(self.manifest)
        for name, key, value in [('base-evm', 'rev', 'main'), ('base-evm', 'version', '1.5'),
                                  ('base-evm', 'default-features', True),
                                  ('revm', 'version', '=44.0.0'), ('revm-base', 'features', ['std']),
                                  ('base-consensus', 'rev', 'main'),
                                  ('base-consensus', 'features', ['std', 'serde']),
                                  ('base-consensus', 'default-features', True),
                                  ('base-consensus', 'package', 'base-common-evm'),
                                  ('base-consensus', 'version', '=1.5.2')]:
            self.manifest = deepcopy(original)
            self.manifest[name][key] = value
            with self.subTest(name=name, key=key), self.assertRaises(ValueError):
                self.validate()

    def test_base_package_source_version_licence_and_addition_cannot_drift(self):
        original = deepcopy(self.metadata)
        for field, value in [('source', BASE_SOURCE.replace(BASE_REVISION, '0' * 40)),
                              ('version', '1.5.2'), ('license', 'Apache-2.0')]:
            self.metadata = deepcopy(original)
            self.package('base-common-evm', '1.5.1')[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'Git package'):
                self.validate()
        self.metadata = deepcopy(original)
        self.add_package('base-unreviewed', '1.5.1', BASE_SOURCE)
        with self.assertRaisesRegex(ValueError, 'Git package'):
            self.validate()

    def test_relaxed_features_fail_even_when_manifest_is_unchanged(self):
        for feature in ['optional_balance_check', 'dev', 'reth', 'metrics', 'asyncdb', 'gmp', 'serde']:
            self.node('revm', '43.0.3')['features'] = [feature]
            with self.subTest(feature=feature), self.assertRaisesRegex(ValueError, 'unreviewed features'):
                self.validate()

    def test_native_secp_feature_cannot_return_transitively(self):
        for key in [('revm', '43.0.3'), ('revm-precompile', '42.0.1'), ('alloy-primitives', '1.7.3')]:
            original = self.node(*key)['features']
            self.node(*key)['features'] = original + ['secp256k1']
            with self.subTest(package=key), self.assertRaisesRegex(ValueError, 'native secp256k1|unreviewed features'):
                self.validate()
            self.node(*key)['features'] = original

    def test_child_cannot_acquire_host_or_network_authority(self):
        bridge = self.add_package('innocent-bridge', '1.0.0', REGISTRY)
        self.add_package('reqwest', '0.12.0', REGISTRY)
        self.node('evm-simulation', '0.1.0')['deps'].append({'pkg': bridge})
        self.edge(('innocent-bridge', '1.0.0'), ('reqwest', '0.12.0'))
        with self.assertRaisesRegex(ValueError, 'forbidden host/network'):
            self.validate()

    def test_reviewed_package_source_and_lock_integrity_are_required(self):
        original_metadata, original_lock = deepcopy(self.metadata), deepcopy(self.lock)
        for key in [('crypto-bigint', '0.5.5'), ('crypto-bigint', '0.7.9'),
                    ('aurora-engine-modexp', '1.3.0'), ('k256', '0.13.4'),
                    ('paste', '1.0.15'), ('syn-solidity', '1.7.3'), ('ark-ff', '0.4.2'),
                    ('revm', '42.0.1')]:
            for mutation in ['metadata-source', 'lock-source', 'checksum', 'version', 'removed']:
                self.metadata, self.lock = deepcopy(original_metadata), deepcopy(original_lock)
                record = next(item for item in self.lock if (item['name'], item['version']) == key)
                if mutation == 'metadata-source':
                    self.package(*key)['source'] = None
                elif mutation == 'lock-source':
                    record['source'] = 'registry+https://example.invalid/index'
                elif mutation == 'checksum':
                    record['checksum'] = '0' * 64
                elif mutation == 'version':
                    self.package(*key)['version'] = '99.0.0'
                else:
                    self.lock.remove(record)
                with self.subTest(key=key, mutation=mutation), self.assertRaisesRegex(ValueError, 'version/source/checksum|Resolved Revm versions'):
                    self.validate()

    def test_extra_vulnerable_version_is_not_covered(self):
        self.add_package('crypto-bigint', '0.5.4', REGISTRY)
        with self.assertRaisesRegex(ValueError, 'versions changed'):
            self.validate()

    def test_aurora_licence_and_other_versions_require_review(self):
        self.package('aurora-engine-modexp', '1.3.0')['license'] = 'CC0-1.0 OR MIT'
        with self.assertRaisesRegex(ValueError, 'Aurora licence'):
            self.validate()
        self.package('aurora-engine-modexp', '1.3.0')['license'] = 'CC0-1.0'
        self.add_package('aurora-engine-modexp', '1.4.0', REGISTRY)
        with self.assertRaisesRegex(ValueError, 'versions changed'):
            self.validate()

    def test_new_direct_numeric_and_modexp_consumers_are_rejected(self):
        self.add_package('new-consumer', '1.0.0', REGISTRY)
        original = deepcopy(self.metadata)
        for key in REVIEWED_PARENTS:
            self.metadata = deepcopy(original)
            self.edge(('new-consumer', '1.0.0'), key)
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'dependency path changed'):
                self.validate()

    def test_existing_direct_parent_version_drift_is_rejected(self):
        self.package('base-execution-eip8130', '1.5.1')['name'] = 'other-base-package'
        with self.assertRaises(ValueError):
            self.validate()

    def test_new_indirect_workspace_consumer_is_rejected(self):
        identifier = self.add_package('new-workspace-consumer', '0.1.0', None)
        self.metadata['workspace_members'].append(identifier)
        self.edge(('new-workspace-consumer', '0.1.0'), ('chain-evm', '0.1.0'))
        with self.assertRaisesRegex(ValueError, 'workspace consumers'):
            self.validate()

    def test_wallet_cannot_reach_old_math_through_an_existing_caller(self):
        self.edge(('wallet-core', '0.1.0'), ('chain-evm', '0.1.0'))
        with self.assertRaisesRegex(ValueError, 'workspace consumers|Wallet'):
            self.validate()

    def test_wallet_must_keep_patched_math_in_its_own_graph(self):
        self.node('wallet-core', '0.1.0')['deps'] = []
        with self.assertRaisesRegex(ValueError, 'Wallet'):
            self.validate()

    def test_obsolete_exception_requires_removal_and_review(self):
        self.metadata['packages'] = [item for item in self.metadata['packages']
                                     if item['id'] != package_id('crypto-bigint', '0.5.5')]
        identifier = package_id('crypto-bigint', '0.5.5')
        self.metadata['resolve']['nodes'] = [node for node in self.metadata['resolve']['nodes']
                                              if node['id'] != identifier]
        for node in self.metadata['resolve']['nodes']:
            node['deps'] = [item for item in node['deps'] if item['pkg'] != identifier]
        with self.assertRaisesRegex(ValueError, 'obsolete'):
            self.validate()


if __name__ == '__main__':
    unittest.main()
