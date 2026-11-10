# Generic source and effect qualification

Status: implementation in progress; the native release security gate remains
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

The compiler artifact owns the exact source contents, settings, selected target,
ABI, storage layout and complete runtime including metadata. Current code must
match those bytes exactly. Initial compilation rejects immutables and linked
libraries. Generic qualification makes no assumption that a particular creation
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
context. A standard slot value alone does not prove dispatch. Unknown delegation,
beacon resolution, selector interception, excessive nesting and writes to proxy
control slots reject, including write-and-restore sequences. Common proxies with
compiler immutable references remain outside the initial compilation subset.

Typed snapshot changes alone do not prove inclusion-time asset or authority
bounds. Generic signing additionally requires an opaque effect envelope derived
from the trusted compiler AST. The admitted grammar assigns constants or exact
calldata values to typed paths, or performs checked bounded integer increments
and decrements. State-derived guards may cause reversion but cannot select
different successful effects. Static nonvirtual internal/private helpers use
bounded parameter binding and the same grammar; recursion, dynamic/super calls,
unknown assembly, unchecked arithmetic and unbounded state-derived writes reject.
Modifiers must have one placeholder and no hidden effects. Relative operations
are displayed as relations, such as an increase by one; the predicted absolute
value is explicitly a snapshot observation. Every relation is cross-checked
against the trusted before/after state, including unchanged writes.

Compiler reproduction is broader than semantic feature qualification. Complex
envelopes currently require solc 0.8.38. Its known-bug record includes optimiser
bookkeeping fixes affecting earlier versions, including ordinary optimised IR
code. Older builds are limited to one unoptimised, non-IR, full-word uint256
setter with scalar inputs, no inheritance, helpers, modifiers or branches. This
excludes the affected dynamic/aliasing/assembly and optimiser features; it does
not establish blanket safety of older compilers. See the
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
proved at the same current snapshot.

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

The explicitly ignored chain-owner integration matrix uses the real packaged
official compiler, generated MPT account/storage proofs, maintained frozen replay
and private candidate construction. It covers direct and exact ERC-1167 calls on
Ethereum mainnet Osaka, Ethereum Sepolia Osaka/Amsterdam, Base mainnet
Cobalt/Denim and Base Sepolia Denim. Base candidates consume genuine proved fee
facts and a prepared frozen native snapshot; their account mode remains the
conservative application-phase projection. These synthetic snapshots establish
the exercised certificate joins, not public-provider availability, canonical
live state, exact signed native settlement or human approval qualification.
