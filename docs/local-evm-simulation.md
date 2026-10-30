# Local EVM simulation qualification

Source builds contain separate local execution helpers and staging support.
They are not yet a chat capability, transaction approval or generic contract signing path.
Generic writes remain unavailable. Unsigned [contract reads](contract-observations.md)
continue to use the separately reviewed configured-RPC `eth_call` boundary.

The helper uses pinned Revm 43.0.3 for Ethereum and native Base 1.5.1 at commit
[`5476069f1eb74a56bccb56fdefd4b4bb0e3cda42`](https://github.com/base/base/tree/5476069f1eb74a56bccb56fdefd4b4bb0e3cda42)
with its compatible Revm 42.0.1. It does not approximate Base with Ethereum/OP
rules, implement an EVM or use hosted simulation. [Dependency qualification](local-simulation-dependency-review.md)
records exact sources, features, licence limits and advisory applicability.

## State and privacy boundary

The host selects an existing configured network/RPC and binds every request to
one header hash. Alloy recomputes the complete header hash. EIP-1186 account and
storage Merkle proofs must match its state root; runtime code must match the
proved code hash. Account absence is explicit and proved, never a missing-data
zero. BLOCKHASH follows hash-verified linked ancestor headers. Chain identity and
canonical block identity are checked before/after execution. Missing proofs,
archive state, canonical hash support or consistency fail the whole run; no
latest/provider/hosted fallback is installed.

Header/canonical-chain identity still relies on the configured provider. This is
not a consensus/light client, finality proof or defence against a provider that
fabricates a self-consistent chain. Canonicality can change after the check.
The RPC sees IP, addresses, code/slot and ancestor queries. Simulation calldata
is executed locally rather than sent by this state adapter, but those queries
can reveal intent; this is neither offline nor metadata-private. Future native
integration must review this outbound scope before any state request. The
developer API itself supplies no native consent and must not be exposed directly
to model tools. No new provider account, credential or payment is required by
the synthetic fixtures; public-provider availability is not guaranteed.

## Fixed admission and resources

The authority-free protocol admits one ordinary unsigned type-2 call: no raw or
signed envelope, key, authentication list, deposit mode, file path, URL, signing
handle or submission capability. Transaction fees, balance and nonce validation
are not relaxed. Delegated account bytecode, CREATE/CREATE2 and SELFDESTRUCT fail.
Only explicit reviewed chain/fork windows are accepted; there is no latest fork.

| Boundary | Current limit |
| --- | --- |
| Execution gas / opcode steps | 1,000,000 / 200,000 |
| Call depth / frames | 16 / 128 |
| State queries / aggregate response bytes | 128 / 4 MiB |
| Host state RPC calls / each IO timeout | 256 / min(remaining budget, 1 second) |
| Pipe frame / return data | 64 KiB / 8 KiB |
| Logs / log data | 64 / 16 KiB |
| Changed accounts / storage words | 128 / 128 |
| Interpreter memory | 4 MiB per context, plus process-wide bound |
| Child physical footprint / wall / CPU | 256 MiB / 15 seconds / 3-second soft, 4-second hard |

macOS uses a mandatory deny-by-default OS sandbox, empty environment, closed
inherited descriptors and independent child resource monitoring. No engine
thread survives return. A result is accepted only after exact request commitment,
block identity and successful child exit checks. Cancellation/limits/pipe faults
discard partial output and kill/reap the helper. macOS footprint monitoring is
sampled, not a hard allocation reservation; CPU and wall controls supplement it.
The concrete RPC adapter checks cancellation after IO as well as before it.
Other platforms fail closed. Staging packages separate pinned helper binaries;
full native/visual qualification and chat enablement remain open.

## Helper identity and signed Base isolation

`TrustedSimulationWorkers::packaged()` accepts no caller path or expected digest.
The desktop build verifies staged helper SHA256 and code-directory hashes and
embeds their exact manifest in the host's immutable Mach-O text section. The
runtime reads that loaded section and derives fixed sibling helper names. Every
trusted launch rejects final-component symlinks, rechecks file contents, and
checks the actual running child's kernel code identity before releasing any
request or signature material. The latter check closes a replacement race between
file verification and execution. Unpackaged developer builds without this section
cannot mint trusted worker capabilities. Raw-path online APIs remain diagnostics
and cannot supply the trusted capability required for qualification.

`scripts/stage_evm_workers.py` builds and signs the two helpers before the desktop
build. Normal source checking does not require staging when the generated manifest
is absent. Release packaging requires it. Bundle verification compares the helpers
against the manifest embedded in that exact host executable; a mutable resource
manifest cannot establish runtime trust. Staging does not launch the app.

Base's native L1 fee depends on the genuine signed envelope, including signature
bytes compressed by the pinned FastLZ algorithm. The online unsigned protocol
therefore cannot provide an exact native Base run. A separate offline protocol
loads and commits a complete proof-checked, host-selected state manifest before
acknowledging readiness; only then may it receive the independently decoded,
recovered and exactly body-bound signed type-2 envelope. It has no network broker,
state-query messages, endpoints or state-fetch callbacks after signature. Unknown
state is terminal, including an unknown slot on a known account; it never expands
the manifest or substitutes zero. Both helpers share the same mandatory sandbox,
resource limits, cancellation and kill/reap lifecycle.

The currently bounded offline certificate scope is one direct zero-native-value
call. Source/profile evidence must independently establish its material effects
and fee insensitivity before approval. A checked all-signature fee ceiling uses
signed length <= unsigned signing length+68 and the pinned algorithm proof
`FastLZLength(n) <= n+ceil(n/32)`, followed by monotone Fjord and operator-fee
formulas. This bounds the frozen proved fee snapshot, not arbitrary later
inclusion-state fees; freshness, funds, nonce and approval bindings remain required.
No dummy or unsigned envelope is presented as exact native execution.

Signed-run child output remains opaque. Validation requires the original host
material certificate, exact preloaded before words, a single direct call and
independently reconstructed native fee/nonce settlement. Logs, return bytes,
account/storage changes, diagnostics and gas/fee figures cannot be projected from
that output. Even a bounded gas number could encode signed bytes. The resulting
host-only receipt exposes only the host-computed genuine transaction hash and the
pre-existing snapshot commitment. It cannot approve or send a transaction.

## Per-network disposition on 30 October 2026

| Network | Explicit current disposition |
| --- | --- |
| Ethereum mainnet | Osaka engine and BPO blob-fee schedule exercised with synthetic worker fixtures. Admission expires 2026-11-01 UTC. Raw execution only; no material-effect/source qualification. |
| Ethereum Sepolia | Osaka before timestamp 1791294816; Amsterdam from that boundary, requiring the exact consensus slot and block-access-list hash. Synthetic slot, state/regular gas, calldata floor/refund and value-log fixtures pass. Same 2026-11-01 block-time horizon; live-state/effect qualification remains open. |
| Base mainnet | Native Cobalt full-envelope execution, L1/operator fees and settlement pass synthetic offline fixtures. The unsigned online protocol preserves missing-envelope rejection. Live source/effect/workflow qualification remains required. |
| Base Sepolia | Native Denim passes the same offline fee/envelope fixtures. No deposit/system-transaction workaround; the same source/effect/workflow requirements apply. |

Ethereum timings/BPO parameters were reviewed against
[pinned Geth chain specs](https://github.com/ethereum/go-ethereum/tree/d68cf359f9876a58fa41c1beab28cd05bc9df9a0/params/chainspecs),
and Base upgrades against the pinned Base implementation. The 2026-11-01 limit
is a maintenance review horizon, not a prediction of the next fork. Extending it
requires source/rules review. Synthetic hashes/state do not qualify a live chain.

## Evidence and enablement gates

Synthetic real-process tests exercise state changes, rollback, false/no-op
returns, untrusted logs, nested/re-entrant calls, public recovery/modexp vectors,
BPO/Amsterdam boundaries, exact consensus header fields, split gas/refunds, missing state, unsupported code, limits and cancellation.
Proof tests exercise identity/root/value tampering, proved absence, exact slots,
header changes, reorg rejection and cancellation before/after RPC return.
They establish those bounded behaviours, not arbitrary semantic completeness.

Execution status, emitted events, decoded function names and changed slots cannot
alone establish assets received, residual approvals, operator/authority effects
or task success. Every result therefore retains `effects_qualified=false` and
cannot become approval/signing authority. Proxy/code/source/ABI correspondence,
independent material-effect observers, current four-chain execution/fee coverage,
packaged native review/per-write authentication, workflow integration with the
[shared nonce owner](evm-nonce-ownership.md) and
durable sequential execution integration remain required. No warning override,
blind-sign route, account upgrade or automatic retry is installed.
