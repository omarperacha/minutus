# Local EVM simulation qualification

Source builds contain separate local execution helpers and staging support.
The helpers alone grant no approval or signing authority. The qualification-only
[generic workflow](generic-contract-qualification.md) independently checks their
evidence through the native controller; release generic writes remain unavailable.
Unsigned [contract reads](contract-observations.md)
continue to use the separately reviewed configured-RPC `eth_call` boundary.

The helper uses pinned Revm 43.0.3 for Ethereum and native Base 1.5.3 at commit
[`35fd079386e8c6ca57e0d354e5cca5a9ad29bf46`](https://github.com/base/base/tree/35fd079386e8c6ca57e0d354e5cca5a9ad29bf46)
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

Proof requests use the pinned header's numeric block height, as specified by
[EIP-1186](https://eips.ethereum.org/EIPS/eip-1186). The returned account proof
must still match that exact header's state root; each storage proof must match
the proved account's storage root. A different block at the same height cannot
substitute different facts. Code requests remain hash-addressed and checked
against the proved code hash, and the final canonical check still requires the
original exact header hash. Numeric proof transport adds no latest-tag or error
fallback. [EIP-1898](https://eips.ethereum.org/EIPS/eip-1898) addresses coherence
of block-selected RPC reads; these additional MPT checks prevent a different-root
response from passing even if a reorganisation occurs between RPC calls.

Ordinary RPC response bodies remain limited to 64 KiB. Only the two standard
`eth_getBlockByNumber`/`eth_getBlockByHash` methods with exactly two parameters
and `fullTransactions: false` have a fixed 512 KiB limit: their responses still
include every transaction hash, and a valid observed header response exceeded
64 KiB. This capacity admits no proof or signing authority; Alloy still
recomputes the complete header hash. Full transaction bodies, other methods and
nonexact parameter shapes retain the ordinary limit. Larger headers reject,
whether their length is declared or streamed. Per-call/phase deadlines and the
separate explicit receipt-acquisition aggregate limit are unchanged.

Header/canonical-chain identity still relies on the configured provider. This is
not a consensus/light client, finality proof or defence against a provider that
fabricates a self-consistent chain. Canonicality can change after the check.
The RPC sees IP, addresses, code/slot and ancestor queries. Simulation calldata
is executed locally rather than sent by this state adapter, but those queries
can reveal intent; this is neither offline nor metadata-private. The native
workflow reviews this outbound scope before any state request. The
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

## Per-network disposition reviewed 10 November 2026

| Network | Explicit current disposition |
| --- | --- |
| Ethereum mainnet | Osaka engine and BPO blob-fee schedule exercised with synthetic worker fixtures. Admission expires 2026-12-01 UTC. Raw execution only; no material-effect/source qualification. |
| Ethereum Sepolia | Osaka before timestamp 1791294816; Amsterdam from that boundary, requiring the exact consensus slot and block-access-list hash. Synthetic slot, state/regular gas, calldata floor/refund and value-log fixtures pass. Same 2026-12-01 block-time horizon; live-state/effect qualification remains open. |
| Base mainnet | Native Cobalt before1793901600 and Denim from that exact mainnet boundary; full-envelope execution, L1/operator fees and settlement pass synthetic offline fixtures. The unsigned online protocol preserves missing-envelope rejection. Live source/effect/workflow qualification remains required. |
| Base Sepolia | Native Denim passes the same offline fee/envelope fixtures. No deposit/system-transaction workaround; the same source/effect/workflow requirements apply. |

Ethereum timings/BPO parameters were reviewed against
[pinned Geth chain specs](https://github.com/ethereum/go-ethereum/tree/ddaa6957127ef0c4b7d41e5cbd9c19e260c0a42f/params/chainspecs),
and Base upgrades against the pinned Base implementation. The 2026-12-01 limit
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


## Trusted frozen host replay

The host can now independently rerun an exact bounded proof witness through the
maintained Revm implementation. This is an explicit execution trust boundary:
`TrustedReplayExecution` is privately constructed from host execution, never from
child JSON. `chain-evm` separately binds its account/code/storage/ancestor inputs
to the exact proved header. `effects_qualified` remains false: decoding source,
layout, native transfers and complete material effects remains a separate owner.

`replay_discover` runs before signature and discovers missing facts by restarting
execution from the unchanged snapshot. It obtains each missing fact from the
trusted proof source and never substitutes zero. The final `replay_frozen` has a
map-only database and no source callback. Both paths share total deadline and
fixed gas, opcode, witness and trace limits. Detailed calls include storage/code
contexts, calldata, return/revert bytes, scheme and ancestor-aware commitment.
The host records all attempted SSTORE keys, including restored/reverted writes,
and bounded KECCAK256 preimages for independent layout mapping. Real committed
CALL value transfers are distinct from DELEGATECALL apparent value and rollback.

Host replay runs on the caller's blocking executor. It has cooperative per-step
cancellation, fixed gas and interpreter memory, rather than the isolated worker's
killable process or physical-footprint boundary. Host application replay rejects
standard precompiles at addresses 1..0x20 and 0x100 before native frame dispatch;
Base native precompile outcomes also reject. Legacy isolated worker crypto
qualification is unchanged. The maintained Rust VM and system runtime remain
inside the explicitly reviewed host execution TCB. No wallet, signing or network
implementation is linked into that replay module. A second invocation of the
same maintained engine is not implementation diversity or a consensus proof.

Base unsigned replay has a distinct application-frame certificate. No invented
envelope or full native transaction result is claimed. It uses maintained Base
ordinary validation, intrinsic gas, warming, frame dispatch, gas refund/floor and
result routines, prepays the independently proved all-signature fee ceiling and
bumps the checked sender nonce. Full envelope validation and fee settlement are
performed only by the subsequent genuine signed native run. Its conservative
prepaid account balance must never be presented as exact native settlement.

Application mode rejects reached BALANCE/SELFBALANCE, nonzero frame value,
fee-system calls, precompiles, creation/destruction and delegated authorisation.
GAS, GASPRICE and fixed-header reads remain ordinary maintained execution: the
body/header and gas accounting are identical across signatures. With no balance
or native-precompile observation and no value transfers, the differing fee
prepayment cannot affect the first differing instruction; the bytecode path,
material effects and gas therefore remain invariant. This is a conservative
executed-path noninterference argument, not a blanket static proof for arbitrary
future state or another body/header.

`OfflineMaterialExpectation::from_trusted_replay` accepts only that opaque Base
certificate and binds the exact request, every ordered call/log, return bytes,
all changed storage owners and all six gas fields. The signed result remains
opaque and must match the certificate plus independently reconstructed exact
native fee/nonce settlement. Its fixed-profile constructor retains the old
single-call/single-target restrictions. No signed state lookup or broker is added.

See [the dated rules review](local-simulation-rules-review-2026-11-10.md) for the
mainnet Denim correction, current official sources and exact dependency limits.
