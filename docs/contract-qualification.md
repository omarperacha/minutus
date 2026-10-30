# Reviewed contract qualification

Declared ABI, successful `eth_call`, events, raw simulation JSON and saved
commitments do not qualify a write. The chain owner constructs non-deserialisable
candidates from canonical proof-checked state, exact source/artifact/deployment
correspondence, independent transaction decoding and complete supported material
effects. Header identity follows the configured RPC; these checks are not a consensus light client.
Only the native controller can request per-write review and fresh vault
password authentication. Neither a plan nor a workflow record approves a write.

Start with an existing selected account already unlocked in Wallet. If it is locked, unlock through Wallet and make a fresh workflow request; the workflow does not unlock before outbound scope consent. This initial readiness requirement does not replace fresh password authentication for every write, and later relocking or account changes remain subject to normal readiness/approval checks.

The new workflow tool is gated to a debug qualification build with packaged attested helpers. Release builds keep it disabled pending [native interaction and human security qualification](contract-native-qualification.md).

## Initial exact source profile

The initial profile is `oz570-solc0838-cancun-erc20-v1`, revision1, artifact
commitment `76df8832488cb0beac36a11b032a1240a39f05202b4c604d580e0485645feef7`.
It is the bundled constructor-only `MinutusQualifiedERC20` wrapper around
OpenZeppelin Contracts5.7.0 at commit
`cab19933c33c2ad1d4c7a84864a3601dddfd16f3`, compiled with official
solc0.8.38+commit.1835d247, Cancun, optimiser200, legacy evmasm pipeline and exact
IPFS/literal-source metadata. The full1764-byte runtime must match Keccak
`0x34ad9dbf5175a1e9da19c8edb9d390a0026ac815cdf159a903a04f53e39fd841`.
No metadata stripping, runtime patching, caller-provided profile catalogue,
selector-only classification, link or immutable substitution is accepted.

Successful canonical EOA type2 CREATE provenance is also required. Transaction
and successful receipt MPT proofs must match the complete canonical creation
header, recovered creator/nonce-derived target, exact creation bytecode,
canonical bounded constructor arguments and expected mint record. The code
account must still have nonce1. This establishes the reviewed initial-supply
invariant used by the source's unchecked recipient addition; matching runtime
alone would not establish that invariant.

Only direct zero-native-value `approve(address,uint256)`,
`transfer(address,uint256)` and `transferFrom(address,address,uint256)` calls with
canonical bool output are covered. Balances and allowances use exact raw units,
without trusting token names or symbols. Approval is zero-first; maximum uint256
is explicitly unlimited. Self-transfers and aliasing are coalesced, unchanged
read-dependent slots still require proofs, finite allowances decrease exactly,
and unlimited allowances remain unchanged. Unknown code, proxies, delegation,
nested calls, extra logs, missing/duplicate/uncovered storage, native balance or
nonce changes, false return, revert and incomplete execution reject admission.

The exact executable runtime has no native-balance, gas/fee, block-context,
external-call, precompile, creation or destruction dependency. This restriction
is essential to the Base material certificate. Broader token/application/swap
families require equivalent independent source and effect coverage; they do not
inherit this profile's authority.

## Per-protocol evidence

Ethereum candidates require the packaged attested isolated worker and exact
local execution. Every material token change, event, canonical true return,
caller nonce and native fee/beneficiary change is independently checked against
proof-checked pre-state. Raw diagnostic worker paths cannot mint qualified
candidates.

Base candidates establish material changes from this fee-insensitive reviewed
source and proved slots before signature material exists. They show a rigorous
maximum total native fee for every canonical signature on the exact unsigned
body at the frozen proof snapshot. Full native Base execution occurs after fresh
native approval/authentication, when the genuine type2 envelope exists, inside a
separate attested strictly offline worker. Before each signed offline-worker run its complete manifest is frozen. An
unknown read is terminal; no broker or endpoint exists in that protocol. The
host requires exact expected material output and independently reconstructed
native settlement. Raw signed worker output, including gas, is never exposed to
model/IPC diagnostics. Dummy/deposit/unsigned envelopes are never represented as
exact ordinary Base execution.

The Base maximum is a bound at the proved snapshot, not an onchain cap on later
L1/operator inclusion charges. Fresh qualification must preserve the exact
unsigned body, source/deployment and relevant token/allowance changes, and stay
within the original reviewed native maximum. Changed material requires fresh
approval. Refreshed and validated certificates share a short monotonic lifetime;
validation never resets their age. Cancellation applies on both networks.

## Receipts and workflows

A successful receipt is recorded separately from task postconditions. The
receipt verifier proves the exact signed body, signer and transaction hash plus
successful receipt at the same transaction index under both canonical MPT roots.
It independently selects the configured RPC's latest header, proves current
profile code and semantic allowance/balance slots, checks the requested goals,
and rechecks deployment/receipt/current canonical identities. A caller-selected
old block cannot be minted as a fresh prerequisite. Balance assertions describe
current goal state; they do not attribute an entire block's balance delta to one
transaction. Saved results never recreate a live proof certificate.

The first host proof-acquisition path is deliberately bounded to64 transactions
per block,4MiB aggregate RPC data and the shared15-second deadline. It requires
raw-transaction RPC support. Unsupported/oversized data leaves the transaction
confirmed but its goal unverified; it does not authorise another send. Both
ordinary and native Base deposit receipts use maintained consensus encoders.

Scoped workflow journals retain immutable network, account, targets, ordered
semantic arguments and profile commitments, plus each fresh candidate's source,
material, financial and observer context. Fresh proof certificates for all prerequisites must share the same state block/hash and are consumed
at next-step claim and again before dispatch after authentication. Dispatch and
the shared account/nonce fence commit in one storage transaction. Current-turn
checks prevent a new plan or model identifier from resetting attempts. A stale,
denied, cancelled, failed or uncertain step stops continuation. There is no
automatic allowance cleanup, duplicate send or atomic-to-sequential fallback.
The initial conservative journal requires every earlier predicate to remain
true at that same state block; workflows that intentionally consume an earlier predicate cannot append
further steps without a separately reviewed dependency model.

## Artifact checks

`python3 scripts/check_contract_profiles.py` checks the immutable manifest,
source bundle, compiler input/settings and complete output artifact pins. CI also
runs `--reproduce`, downloading the exact official build-only compiler into a
temporary directory, checking its published SHA256 and reproducing every output
under pinned Node22.23.4. A local verified compiler can be supplied with
`--compiler`. No compiler or source-fetching dependency is added to the runtime.

These checks and synthetic fixtures establish bounded implementation evidence.
Native interaction qualification and final human security/release review remain
separate gates; no live funded transaction is implied.
