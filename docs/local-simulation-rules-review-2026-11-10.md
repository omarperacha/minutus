# Local execution rules review — 10 November 2026

The reviewed maintenance deadline is now 1 December 2026 UTC. This does not
predict the next fork, select a latest engine, or qualify live RPC consensus and
finality. Unknown contexts and the exact renewed deadline reject.

## Sources and rule selections

| Network | Current reviewed rules |
| --- | --- |
| Ethereum mainnet | Osaka from `1764798551`; BPO1 from `1765290071`, BPO2 from `1767747671`, update fraction `11684671` afterwards. |
| Ethereum Sepolia | Amsterdam from `1791294816`; exact consensus slot and block-access-list hash remain mandatory. |
| Base mainnet | Cobalt from `1790791200` until `1793901600`; Denim from that exact boundary. |
| Base Sepolia | Denim from `1792605600`. |

The official [Geth mainnet](https://raw.githubusercontent.com/ethereum/go-ethereum/ddaa6957127ef0c4b7d41e5cbd9c19e260c0a42f/params/chainspecs/mainnet.json)
and [Sepolia specifications](https://raw.githubusercontent.com/ethereum/go-ethereum/ddaa6957127ef0c4b7d41e5cbd9c19e260c0a42f/params/chainspecs/sepolia.json)
were reviewed at immutable commit `ddaa6957127ef0c4b7d41e5cbd9c19e260c0a42f`, dated 10 November 2026. No mainnet Amsterdam activation is configured in
those sources. Existing pinned Revm 43.0.3 implements the selected Ethereum
variants, including the already exercised Amsterdam split gas/floor/refund rules.

The official [Base announcement](https://www.base.org/blog/denim), 5 November,
[status record](https://status.base.org/) and [upgrade documentation](https://docs.base.org/upgrades/denim/overview)
confirm mainnet Denim activation at 18:00 UTC on 5 November (`1793901600`).
Previous application admission selected Cobalt indefinitely; that is corrected,
independently of the maintenance deadline. The [pinned Base configuration](https://github.com/base/base/blob/35fd079386e8c6ca57e0d354e5cca5a9ad29bf46/crates/common/chains/src/config.rs)
contains both network activation times.

Denim's [BaseTime implementation](https://github.com/base/base/blob/35fd079386e8c6ca57e0d354e5cca5a9ad29bf46/crates/common/evm/src/base_time.rs)
stores the subsecond component in proved predeploy storage. EVM TIMESTAMP stays
seconds-based. Optional RPC timestampMs is not a substitute for authenticated
state and is not injected into the EVM context. Transaction-on-proved-state mode
does not reapply block-level BaseTime deployment or metadata deposits.
The official [Everest overview](https://docs.base.org/upgrades/everest/overview),
read 10 November, lists planning/TBD for both networks. No Everest, EIP8430,
deposit, authorisation or account-upgrade transaction mode is admitted.

## Exact source and dependency update

The native Base pin is now 1.5.3 at immutable commit
`35fd079386e8c6ca57e0d354e5cca5a9ad29bf46`; the annotated tag object is
`0e50ccaff86b5f1c1606c6dac53ab7b3fc0a979d`. Revm remains42.0.1 for Base
and 43.0.3 for Ethereum. Every file in all 12 reachable Base package directories
(Rust, manifests, bytecode/assets) is byte-identical to 1.5.1, and the root MIT
licence is unchanged. Only excluded optional Reth source tags differ in inherited
metadata. The 12 Git package versions/source references and engine identity tags
were updated; no registry version/source/licence/feature drift occurred across
that pin replacement. Exact offline metadata and the strict source policy pass.

Base's [release note](https://github.com/base/base/releases/tag/v1.5.3) references
[Reth #28264](https://github.com/paradigmxyz/reth/pull/28264), a cached trie-iterator
seek/incremental-root fix. Reth trie construction is outside this native EVM leaf;
this application uses separately checked Alloy MPT verification. An initial Git
source fetch hit sandbox DNS and an uncompleted escalation. The subsequent
purpose-built isolated Cargo metadata fetch used the existing Cargo approval and
completed the exact-source review. No missing source-access blocker remains.
Its temporary unconstrained registry resolution was used only to inspect source;
those registry versions were not copied into the application lock.

Separately, native Base receipt/transaction serde introduces only dashmap 6.2.1
(MIT) and hashbrown 0.14.5 (MIT OR Apache-2.0) and forwards serde into the native
Revm 42 generation. Exact sources, checksums, licences, features and parent paths
are reviewed and enforced in the dependency policy. Complete packaged MIT notices
are preserved in the shipped tree. No fee/balance validation switch, licence
allowance or dated advisory exception is broadened. See the [dependency review](local-simulation-dependency-review.md).

## Verification

Tests cover exact mainnet Denim and Sepolia boundaries, all four current network
selections, the last accepted review second and exact expiry rejection. Actual
native transaction/application comparisons additionally cover Cobalt, mainnet
Denim, Sepolia Denim, two genuine low-s signatures per body, nested calls, 1167
delegate forwarding, calldata floors and independent exact fee/nonce settlement.
These are synthetic fixtures: no real wallet, key, RPC or submission is involved.
