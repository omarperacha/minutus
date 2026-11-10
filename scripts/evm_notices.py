"""Exact reviewed notices shipped with the host and isolated EVM helpers.

Changing the closed notice set or its hashes requires the corresponding source
and licence review. Runtime manifests or environment values cannot replace pins.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RESOURCE_NAME = "third-party-notices"
MANIFEST_SHA256 = "3edd75dda2a7558ff662360571e31737181a167b80f8803135c034a0354c1c30"
NOTICES = (
    ('contract-profiles/erc20-oz-5.7.0/OpenZeppelin-MIT.txt',
     'crates/chain-evm/profiles/erc20-oz-5.7.0/OpenZeppelin-MIT.txt',
     '20aebc68b11c063133aa2af0ef4bb29875477c6d16d715718f0daec563938b84'),
    ('local-simulation/Alloy-MIT.txt',
     'docs/third-party/local-simulation/Alloy-MIT.txt',
     '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    ('local-simulation/Base-MIT.txt',
     'docs/third-party/local-simulation/Base-MIT.txt',
     '140d1113c02e578e7d706c960840e8ee791310d3312e1188b5dc94bf7a837ef9'),
    ('local-simulation/CC0-1.0.txt',
     'docs/third-party/local-simulation/CC0-1.0.txt',
     'a2010f343487d3f7618affe54f789f5487602331c0a8d03f49e9a7c547cf0499'),
    ('local-simulation/DashMap-MIT.txt',
     'docs/third-party/local-simulation/DashMap-MIT.txt',
     '16692e8cee4aa06e3913787497eba2d47c42002014136f5da67be6ee640e28a3'),
    ('local-simulation/Hashbrown-MIT.txt',
     'docs/third-party/local-simulation/Hashbrown-MIT.txt',
     'ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2'),
    ('local-simulation/README.md',
     'docs/third-party/local-simulation/README.md',
     '7c8a507d909dc54a9f593fc80fc611016fda240f5ca3317ac8b8a6ae45db3dc6'),
    ('local-simulation/Revm-MIT.txt',
     'docs/third-party/local-simulation/Revm-MIT.txt',
     '8946bedbf7b19245da61f429ac90b03dd6b6c69e1e2b98988aa1793958c54e2f'),
    ('solidity-compiler/README.md',
     'crates/solidity-compilation/notices/README.md',
     'd9ec899a31d2f06a86761d6b916e72e8021b5af5dfa1d9fb4b7f958d655cc80c'),
    ('solidity-compiler/Solidity-GPL-3.0.txt',
     'crates/solidity-compilation/notices/Solidity-GPL-3.0.txt',
     '0ae0485a5bd37a63e63603596417e4eb0e653334fa6c7f932ca3a0e85d4af227'),
    ('solidity-compiler/compiler.swift',
     'crates/solidity-compilation/native/compiler.swift',
     '88c686da39a76ea4bbf594c4284a59be72d32d63c960f692c403148c00a6d19a'),
    ('solidity-compiler/build.py',
     'crates/solidity-compilation/native/build.py',
     '36345dcb1aefaeaa23ff074901b0575141eadfe0de1be9dbf4ead348cc60ff43'),
    ('solidity-compiler/SolidityCompiler.entitlements.plist',
     'crates/solidity-compilation/native/SolidityCompiler.entitlements.plist',
     '518f9a6c9bd10a1a56073039ddac9a7427745bcb5c470c4b33c814fadfb62d22'),
    ('solidity-compiler/compiler-catalogue.json',
     'crates/solidity-compilation/compiler-catalogue.json',
     '57196e765cc87b6a82a71332fe9dcba140fd55e03000f7b3b347f4fadf818166'),
)


def _read(root, relative):
    current = root
    for component in ("", *Path(relative).parts):
        current = current / component
        if current.is_symlink():
            raise ValueError(f"Third-party notice path is indirect: {relative}")
    if not current.is_file():
        raise ValueError(f"Required third-party notice is missing: {relative}")
    with current.open("rb") as stream:
        value = stream.read(64 * 1024 + 1)
    if len(value) > 64 * 1024:
        raise ValueError(f"Third-party notice exceeds its bound: {relative}")
    return value


def _digest(value):
    return hashlib.sha256(value).hexdigest()


def verify_resource_tree(directory):
    """Verify a bounded complete notice tree, including its reviewed SHA root."""
    expected_manifest = "".join(f"{digest}  {destination}\n"
                                for destination, _, digest in NOTICES).encode("ascii")
    if _digest(expected_manifest) != MANIFEST_SHA256:
        raise ValueError("Reviewed third-party notice manifest root changed")
    manifest = _read(directory, "manifest.sha256")
    if manifest != expected_manifest or _digest(manifest) != MANIFEST_SHA256:
        raise ValueError("Third-party notice manifest differs from its reviewed SHA root")
    expected = {"manifest.sha256", *(destination for destination, _, _ in NOTICES)}
    actual = set()
    for path in directory.rglob("*"):
        if path.is_symlink():
            raise ValueError("Third-party notice tree contains an indirect path")
        if path.is_file():
            actual.add(path.relative_to(directory).as_posix())
    if actual != expected:
        raise ValueError("Bundled third-party notice file set changed")
    for destination, _, digest in NOTICES:
        if _digest(_read(directory, destination)) != digest:
            raise ValueError(f"Third-party notice content changed: {destination}")


def verify_notice_sources(root=ROOT):
    """Check canonical source notices, tracked bundle copies and both configs."""
    for _, source, digest in NOTICES:
        if _digest(_read(root, source)) != digest:
            raise ValueError(f"Reviewed source notice changed: {source}")
    for config in ("tauri.conf.json", "tauri.evm-workers.conf.json"):
        resources = json.loads(_read(root, f"src-tauri/{config}"))["bundle"]["resources"]
        if not isinstance(resources, list) or resources.count(RESOURCE_NAME) != 1:
            raise ValueError(f"Third-party notices are not bundled by {config}")
    verify_resource_tree(root / "src-tauri" / RESOURCE_NAME)


def verify_bundled_notices(application):
    """Verify actual resource bytes; no source-side notice copy can substitute."""
    current = Path(application)
    for component in ("", "Contents", "Resources", RESOURCE_NAME):
        current = current / component
        if current.is_symlink():
            raise ValueError("Bundled third-party notice root is indirect")
    verify_resource_tree(current)
