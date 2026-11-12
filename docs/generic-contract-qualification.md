# Generic source and effect qualification

Status: bounded generic qualification implemented; the native release security gate remains
closed. This document describes the generic boundary and does not qualify a
release, provider, real wallet interaction or funded transaction.

Generic admission binds an independently selected canonical function to locally
reproduced Solidity source, current proved runtime and a complete bounded EVM
transition. It does not require a maintained contract or function catalogue.
Compiler builds and network forks remain explicitly pinned protocol dependencies.

The trusted local compiler and maintained Revm replay are part of the execution
trusted computing base. A sandbox image identity check does not prove execution
correctness. Untrusted worker JSON, source-service verification booleans, ABI
names, event names and caller-supplied digests cannot construct qualification
evidence. State and code must be independently checked against one canonical
header's roots, within the documented RPC-provider trust boundary.

The packaged compiler and EVM helpers use distinct fixed-role XPC launchers.
Each launcher attests its strict worker and seals itself before input; exact
broker pins are read from the loaded host. Sensitive data uses authenticated
anonymous pipes, with actual child-reaping evidence required for cleanup. Missing
offline signed cleanup quarantines further EVM acquisition in the process.
See [worker isolation](local-evm-simulation.md).

The compiler artifact owns the exact source contents, settings, selected target,
ABI, storage layout and complete runtime including metadata. Current code must
match those bytes exactly. Compiler-declared immutable references may bind exact
32-byte runtime words: every occurrence of an AST declaration must agree, each
word must be canonical for its compiler-resolved scalar type, and every byte
outside those declared spans must match. Ordinary compilation still rejects
unbound references; linked libraries remain unsupported. Immutable values do not
establish constructor invariants. Generic qualification makes no assumption that a particular creation
transaction established a token supply invariant. The actual current state and
trusted replay determine the transition.

The initial structural storage interpreter covers integer, boolean, address and
fixed-byte primitives, packed fields, structs, fixed arrays, primitive-key
mappings with independently recomputed observed hash preimages, and bounded
dynamic arrays. Dynamic bytes/string values support canonical short and long
encoding, exact UTF-8 for strings, zero padding and fully proved old/new data
chunks through 1,024 bytes. Shrinking long values requires cleared old tails.
All changed bits must be covered, including adjacent packed
fields. Unknown slots, unresolved keys, unsupported storage encodings, invalid
primitive values, ambiguous paths and exhausted bounds reject admission.
Arbitrary unstructured assembly slots are not silently treated as ordinary
fields. Source-field predicates use the same bounded layout
resolver over independently proved state.

Field paths and event signatures are source declarations. A field named
`balance`, `owner` or `allowance` does not acquire token, ownership or spending
authority semantics through its name. A `Transfer` log, a true return value or a
successful receipt alone does not establish a business goal. Exact raw words,
decoded primitive changes, raw logs, return bytes, code/storage call contexts and
native settlement remain distinct evidence. Explicit typed conditions determine
the reported workflow observations.

Exact standard ERC-1167 runtimes expose a fixed implementation address. ERC-1967
delegation additionally requires source correspondence, proved implementation,
admin and beacon controls, and the actual trusted invocation's code/storage
context for unsigned observation. A standard slot value alone does not prove dispatch. Unknown delegation,
beacon resolution, selector interception, excessive nesting and writes to proxy
control slots reject, including write-and-restore sequences. Signing accepts
direct code and exact fixed ERC-1167 dispatch; mutable ERC-1967 delegation remains
excluded regardless of successful source reproduction.

Typed snapshot changes alone do not prove inclusion-time asset or authority
bounds. Generic signing additionally requires an opaque effect envelope derived
from the trusted compiler AST. The admitted grammar assigns constants or exact
calldata values to typed paths, or performs checked bounded integer increments
and decrements. State-derived guards may cause reversion but cannot select
different successful effects. Statically resolved internal/private/public helpers use
bounded parameter binding and the same grammar. Compiler-resolved inheritance,
virtual overrides, explicit base calls and lexical `super` calls use the selected
contract's C3 order. Dynamic function pointers, recursion, unknown assembly,
unchecked arithmetic and unbounded state-derived writes reject.
Modifiers must have one placeholder and no hidden effects. Relative operations
are displayed as relations, such as an increase by one; the predicted absolute
value is explicitly a snapshot observation. Every relation is cross-checked
against the trusted before/after state. Every attempted persistent write must
belong to an admitted writable physical word, including unchanged and restored
writes. Addressing reads and container lengths do not grant write permission.
Unqualified virtual modifiers follow compiler override declarations; qualified
base modifiers remain rejected. The structured tier also permits `for` loops
with state-independent conditions and checked primitive local copies/updates,
bounded to 32 iterations per loop within the shared step budget. State-derived
bounds, aggregate mutations, break, continue and unchecked blocks reject.

High-level external calls may compose a bounded source graph when their targets
and arguments are state-independent and every business target is explicitly
declared in the approved workflow. Private compiler-bound immutable targets are
supported. Each callee needs its own same-header reproduced source and immutable
dispatch proof. Relations name both storage and code owners; all owners have
disjoint typed write paths. The maintained replay must match the complete exact
source-derived invocation tree, including callers, calldata, values, call kinds,
success and fixed-proxy forwarding. An observed call trace cannot create the
source graph. Limits are 16 business/code contracts, 32 invocations and relations,
eight logical levels and 4,096 shared analysis steps. Low-level calls, ignored
failure, state-derived routing, return-value-dependent effects, overlapping
writers and application access to reserved native/system targets reject.

Compiler reproduction is broader than semantic feature qualification. Complex
envelopes use solc 0.8.38 or later. Earlier official 0.8.x builds have a separate
restricted scalar tier: original compiler input must explicitly disable the
optimiser, omit custom optimiser details and experimental settings, and use the
legacy pipeline. The original settings are retained because compiler metadata
can omit distinctions relevant to historical bugs. This tier permits elementary
ABI/storage fields, elementary-key uint mappings, acyclic scalar helpers,
guard-only modifiers and fixed high-level calls. It checks external interface
arguments and results even when results are ignored. Inheritance, immutables,
reference types, aggregate ABI/storage, loops, assembly, unchecked blocks,
function values, delete and transient storage remain excluded in that tier.
Optimised historical artifacts may reproduce but cannot qualify. These exclusions
remove reviewed known-bug triggers; they do not establish blanket safety of older
compilers. See the
[official known-bug record](https://docs.soliditylang.org/en/v0.8.38/bugs.html).

Mutable ERC-1967 dispatch is rejected by the signing envelope even when its
current source and implementation are known. A normal type-2 transaction has no
implementation-hash guard, so an upgrade between submission and inclusion could
replace its behaviour. Exact minimal proxies retain fixed dispatch. The generic
effect envelope establishes supported source-field relations, not arbitrary
economic meaning, aggregate historical spending or future third-party uses of
those fields. Opaque asset-spending routers remain rejected.

Source-only preflight contains no manufactured transaction or call trace. It can
qualify direct code and exact minimal-proxy correspondence. A proved ERC-1967
potential implementation can supply an unsigned ABI candidate; it does not
select the proxy's storage layout or prove delegation. The selected invocation
must independently execute that implementation, and selector interception
rejects. All potential implementation code remains locally reproduced and
proved at the same current snapshot. Optional source inspection can return direct
high-level call requirements for a hypothetical invocation. It performs local
source analysis only, acquires no callee source automatically and grants no
signing, wallet or scope authority. The caller must inspect and explicitly declare
each required business target before requesting a write.

Ethereum native value and internal native transfers are recorded separately
from apparent delegate-call value. Proved empty-code recipients need no ABI;
unknown nonempty executed code still rejects. Committed transfers exclude
reverted frames. The selected EOA's maximum total debit includes its transaction
value and maximum protocol fee. Complete native-account changes also remain
visible, retaining fee beneficiaries and address aliases. The initial effect
grammar admits signed top-level native value; business internal transfers remain
rejected until their onchain amount/target bounds can be proved.

Base application material and full native signed execution are separate
certificates. Signature-dependent fee deductions occur before application
execution. A generic pre-signature material preview therefore requires a sound
fee-independence boundary, rather than a fabricated signed envelope, a deposit
transaction or another network's execution rules. The conservative subset has
zero native value, no internal native transfers, no executed balance inspection
and no unsupported system-contract access. Exact signed execution later uses a
fully frozen proof map and the full maintained Base handler. Every material
effect must match approval; all post-signature child-selected failures project to
one fixed rejection. Physical timing channels are not claimed to be eliminated.
The Base application-phase native account projection uses conservative
all-signature prepayment, explicitly labelled as remaining-balance bounds. It is
not displayed or reported as exact signed fee settlement. Ethereum projections
are exact maintained transaction executions at their frozen snapshots. Neither
projection guarantees inclusion-time balances.

Snapshot predictions do not guarantee inclusion-time behaviour. Every write
retains independent native approval, fresh authentication, current source/state
refresh, financial bounds, cancellation, nonce ownership and one-attempt durable
submission. Historical receipt inclusion and fresh current predicates remain
separate certificates. Neither grants signing authority or guarantees finality.

Each current-state security phase selects one opaque live cohort inside the
chain owner. Source/state correspondence, replay, prerequisite fields and
predicted completion use that exact canonical block. Its 30-second monotonic
freshness clock starts before latest-header selection and is never reset by
later proof completion. Normal head advancement is permitted while the selected
anchor remains canonical and the head has not regressed below it; reorganisation,
expiry and missing proofs still reject. A caller-supplied historical block cannot
mint a live cohort or current prerequisite authority.

After signing authentication and again before transport, refresh selects a new
live cohort and proves both execution and prerequisites there. The refreshed
binding preserves the original approved candidate digest and exact reviewed
transaction body. Candidate and condition block identities must agree; neither
independent latest reads nor a larger evidence lifetime repair a mismatch.
Qualification errors identify the failed phase. `SnapshotMismatch` also covers
acquisition or state-binding failures and alone does not establish changed source.

Candidate preparation retains one proof reader across source matching, relation
planning and trusted replay, with its original header, verified caches, request
counter and acquisition deadline. Current prerequisites may reuse that candidate's
opaque proved prestate; predicted completion overlays only trusted replay writes.
Missing ancillary accounts or slots require independent proofs at the exact same
root. Contradictory records, unproved zero values, changed source/control identity,
reorganisation and expired evidence reject. This reuse grants no cross-block
authority: post-inclusion completion and final goals still acquire their own
current proofs.

The host claims the current user turn before initial wallet readiness. If the
selected account is locked, the shared native unlock prompt opens in chat before
account-bound outbound scope consent. Cancelling unlock retires the attempt
without starting source/RPC acquisition; a new attempt needs a deliberate new
user message. Unlocking is separate from exact transaction approval and the fresh
signing password required after approval for every write.

Lifecycle reporting distinguishes initial wallet readiness and unlock, outbound
scope consent, source preflight, exact transaction review, fresh signing
authentication, signature return, submission admission and observed inclusion.
Only host facts from the same attempt and step establish which boundaries were
reached. Initial unlock or outbound consent does not establish transaction
approval or signing authentication. A failure code such as `MissingState` or
`SnapshotMismatch` identifies neither the native interaction nor whether signing
or submission started; unavailable facts remain unknown. A missing returned
signature does not establish that signing never began. Likewise, an acknowledged
submission or confirmed receipt must remain reported when a later proof fails,
even if no declared completion certificate was produced.

Earlier source or field reads remain historical evidence at their listed blocks.
They do not establish unchanged source, unchanged storage or a current value after
a failed workflow. Failure reporting creates no new attempt, retry, replacement
or cleanup authority. Exact transport admission and durable journal facts retain
any possible-send outcome for reconciliation.

The explicitly ignored chain-owner integration matrix uses the real packaged
official compiler, generated MPT account/storage proofs, maintained frozen replay
and private candidate construction. It covers direct and exact ERC-1167 calls on
Ethereum mainnet Osaka, Ethereum Sepolia Osaka/Amsterdam, Base mainnet
Cobalt/Denim and Base Sepolia Denim. Compositional fixtures also join separately
proved business owners and fixed proxy caller/callee contexts; historical scalar
fixtures use the genuine official compiler rather than hand-written ASTs. Base candidates consume genuine proved fee
facts and a prepared frozen native snapshot; their account mode remains the
conservative application-phase projection. These synthetic snapshots establish
the exercised certificate joins, not public-provider availability, canonical
live state, exact signed native settlement or human approval qualification.
