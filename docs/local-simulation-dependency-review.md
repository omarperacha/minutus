# Local simulation dependency review

Originally reviewed on 2026-10-30; exact pin and native receipt serde scope updated on 2026-11-10. The current Base pin below supersedes the original 1.5.1 pin for the bounded B1 local execution worker. This is a source,
dependency and applicability review, not an external audit or a claim that an
upstream vulnerability is fixed. Both package exceptions below require re-review
before **2027-01-30 UTC**; the dependency check rejects that date and later dates.
This maintenance deadline is separate from the worker's shorter fork admission
windows.

## Exact engine boundary

The selected registry engines are Revm `43.0.3` for Ethereum and `42.0.1` for
Base. Base uses the native `base-common-evm` `1.5.3` implementation at
[`35fd079386e8c6ca57e0d354e5cca5a9ad29bf46`](https://github.com/base/base/tree/35fd079386e8c6ca57e0d354e5cca5a9ad29bf46),
rather than approximating Base with an OP handler. The reviewed features are
`std`, `portable`, `blst`, `c-kzg` and `memory_limit`, with default features
disabled. Signature recovery uses upstream's Rust `k256` implementation;
`secp256k1` is excluded from both engines and their precompile/Alloy feature
unification. This avoids adding the unreviewed native `secp256k1` `0.31.1` and
`secp256k1-sys` `0.11.0` CC0 path.

The exact Git package set is checked, including `base-metrics` with `std` only.
The metrics, reth, async database, development and relaxed validation features
are excluded. Base's mandatory EIP-8130 implementation dependencies remain in
the graph even though B2 envelopes and authorisation lists are not admitted by
the worker. The native Base fee handler currently rejects the unsigned request
without an envelope. This dependency review does not bypass that admission gate.

The worker's dependency closure excludes wallet, filesystem, permissions,
application storage, HTTP clients and asynchronous network runtimes. The parent
owns RPC and verified state acquisition. The child receives public EVM values
through a bounded protocol, with no secret-key or signing API. A request may
contain private pre-broadcast intent; local execution is not a promise that all
calldata is already public. Resource limits and OS confinement are separate
requirements from the dependency exceptions.

## RUSTSEC-2026-0387 applicability

[`RUSTSEC-2026-0387`](https://rustsec.org/advisories/RUSTSEC-2026-0387.html)
reports timing variability in operations on secret integers and gives
`crypto-bigint >=0.7.6` as the patched range. The selected `0.5.5` remains affected.
The exception concerns the current recovery and verification use of public
signature scalars and points; it does not qualify secret signing with that
version or establish constant-time behaviour.

The older chain is `crypto-bigint 0.5.5` through `elliptic-curve 0.13.8`,
`ecdsa 0.16.9`, `primeorder 0.13.6`, `k256 0.13.4` and `p256 0.13.2`. Its direct
outer consumers are the two exact `revm-precompile` versions,
`base-execution-eip8130 1.5.3`, `alloy-consensus 2.5.0`, `alloy-eip7702 0.6.3`
and `alloy-primitives 1.7.3`. Existing Alloy verification types also make the
older arithmetic reachable from `chain-evm`, `x402-client`, `app-core` and
`zeta-desktop`; it is not exclusively a child dependency. These owners construct
public payloads, hashes and signatures, while wallet signing stays inside
`wallet-core`.

The wallet's selected signing path uses `k256 0.14.0`, `elliptic-curve 0.14.1`
and patched `crypto-bigint 0.7.9`. Its transitive metadata closure must contain
that exact patched bigint and must not reach `0.5.5`. The graph check is
conservative, including metadata dependencies for optional and other-target
paths; it is not a substitute for reviewing new use of signing APIs within an
existing allowed consumer. Moving secret arithmetic into any old consumer
invalidates the applicability basis even if package names remain unchanged.

Switching on AWS-LC P-256 does not remove the mandatory older `k256`/`p256`
dependencies in this pinned upstream graph. Forcing a semver-incompatible bigint
upgrade into those libraries would require a separate cryptographic port and
review. No such patch is included. The preferred removal is a supported upstream
verification-library migration; retain the expiry until that is qualified.

## Aurora CC0 package qualification

The exact new licence scope is `aurora-engine-modexp =1.3.0`, registry checksum
`5188e264926edbd2e90d61bf8b33aa3471db8acdf427fa37946f9c82898fe502`.
Its packaged manifest declares `CC0-1.0`; packaged VCS metadata identifies
[`aurora-is-near/aurora-engine` at `4173da04948a1b2f0f36760fec9d5cd525d6f245`,
`engine-modexp`](https://github.com/aurora-is-near/aurora-engine/tree/4173da04948a1b2f0f36760fec9d5cd525d6f245/engine-modexp).
The package supplies byte-input modular exponentiation for the two pinned Revm
precompile implementations. Those are its only reviewed direct consumers.
The GMP feature does not eliminate this mandatory dependency and would add a
native dependency surface.

This review supports a new exact-package CC0 exception under the existing
[licence policy](license-policy.md); the three earlier wallet exceptions do not
extend to Aurora. CC0 waives copyright and related rights and provides a fallback
licence. Its [section 4(a)](https://creativecommons.org/publicdomain/zero/1.0/legalcode.en)
excludes patent and trademark rights. Accepting this package's licence limitation
does not establish freedom from third-party patent claims, and does not certify
its implementation. Keep its licence declaration and SBOM entry in distribution
notices. CC0 remains outside the global allowlist.

## Compile-time maintenance scope

The EVM additions also introduce `paste 1.0.15` through the exact Alloy and
Arkworks parents recorded in the [maintenance exception](dependency-maintenance.md).
Their versions, registry sources and archive checksums share this review's
2027-01-30 deadline. The original tokeniser constraints remain intact. The
maintenance advisory is separate from the bigint vulnerability and Aurora licence
qualification.

## Enforced review limits

`scripts/simulation_dependency_policy.py`, called by the normal dependency gate,
checks the exact Base revision, package set and features; the reviewed registry
versions, sources and lockfile checksums; the direct older-math and modular
exponentiation consumers; the five workspace ancestors of affected bigint; wallet
separation; and the UTC expiry. An upgrade, additional consumer, native signature
backend, source replacement, obsolete exception or deadline requires re-review.
Cargo's locked fetch verifies archive integrity against the lockfile; this check
binds that lockfile to the reviewed checksums. It does not replace cargo-deny's
other advisory, licence, source and integrity checks.

Focused mutation tests are in `scripts/test_simulation_dependency_policy.py` and
run with the existing Python test discovery. An exact advisory ignore in
`deny.toml` is defensible only with these applicability constraints. It must not
be described as a vulnerability repair, a blanket advisory waiver or permission
to expose wallet secrets to the worker.


## Native receipt serde and exact Base update, 10 November 2026

The selected Base 1.5.3 commit is 35fd079386e8c6ca57e0d354e5cca5a9ad29bf46.
Every file in the 12 reachable Base package directories, including Rust code,
package manifests and bytecode assets, is byte-identical to the earlier 1.5.1
checkout. The root MIT licence is also identical. Changed optional Reth source
tags remain outside the admitted feature graph. Registry versions, sources,
licences and feature sets were preserved exactly across the Base pin update.

Separately, maintained typed Base transaction/receipt decoding requires the
explicit `base-common-consensus/serde` feature. This forwards serde to the
existing native Revm 42 generation and existing codec types; Revm 43 retains its
old feature set. It adds only `dashmap 6.2.1` (MIT, features inline/serde,
registry SHA-256 e6361d5c062261c78a176addb82d4c821ae42bed6089de0e12603cd25de2059c)
and `hashbrown 0.14.5` (MIT OR Apache-2.0, features inline-more/raw,
registry SHA-256 e5274423e17b7c9fc20b6e7e208532f9b19825d82dfd615708b70edd83df41f1).
Their exact licence text is preserved in the shipped notice tree, selecting
Hashbrown's MIT option. The strict guard pins versions/checksums, features,
licences and the exact `reth-primitives-traits0.6.0 → dashmap → hashbrown` path.
Mutation tests reject missing serde, spread to Revm 43 and fee-disable features.
No existing advisory exception, deadline, network boundary or validation switch
is widened. The refreshed advisory result and the separate SQLite repair below
supersede the earlier offline-only advisory check. Source access and downloaded
candidates are not treated as an external security audit.

## Fresh bundled SQLite advisory repair, 10 November 2026

Refreshing RustSec found [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html),
issued on 9 November. A crafted database and sibling WAL can expose process
memory through the vulnerable bundled SQLite versions. The workspace retains
its `bundled` SQLite configuration and updates only `rusqlite 0.40.2 → 0.41.0`
and `libsqlite3-sys 0.38.2 → 0.39.1`. The latter bundles SQLite 3.54.1, within
RustSec's patched range. Both packages retain MIT licensing; no dependency
feature, source override, advisory ignore or existing exception changes.

The [rusqlite 0.41.0 release](https://github.com/rusqlite/rusqlite/releases/tag/v0.41.0)
changes some statement metadata APIs, so the storage owner separately checks
application compatibility. Locked metadata confirms precisely those two package
replacements. `cargo deny --locked --offline check --hide-inclusion-graph` passes
advisories, bans, licences and sources against the freshly updated database;
existing duplicate-version warnings remain. This records a dependency repair,
not proof that arbitrary database inputs are safe.
