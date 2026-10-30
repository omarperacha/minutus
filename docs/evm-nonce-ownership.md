# Shared EVM nonce ownership

Wallet and chat transfers now claim an exact network/account/nonce before fresh
password authentication. Multiple prepared candidates can exist, but only one
live owner can authenticate and dispatch that nonce. Native approval, exact
payload verification, fresh password and wallet epoch checks remain required;
the nonce claim itself grants none of those authorities. Generic contract writes
remain unavailable. A future workflow must use this same owner boundary.

The host binds the candidate digest and native review identity to an opaque,
non-cloneable claim. SQLite schema 23 gives every owner a non-reused database
fence, canonicalises account spelling, and permits only one active owner of each
network/account/nonce. A signed hash is then bound immutably. Liveness is checked
around pre-signing revalidation; the final exact-owner compare-and-swap precedes
transport admission. Ownership moves into the actual blocking worker, so dropping
an awaiting UI future cannot release a worker that may still send.

## Definitely unsent versus possible send

Before dispatch admission, denial, authentication/revalidation failure or normal
guard drop retires only that exact unsent owner. Preparation/recovery can retire
reserved work atomically; an older owner then fails the final fence. Its identity
cannot delete or dispatch a newer reservation, even for identical signed bytes.

Once the dispatch fence marks a possible send, guard drop, restart, a missing
transaction, provider failure, failed receipt or uncertainty acknowledgement do
not free the nonce. A failed receipt still consumes the EOA nonce. The one narrow
cleanup path requires the final admission callback to have returned false, before
the broadcaster can call send. A timeout, worker panic or send error is not this
proof. Cancellation can make the callback return false even after its wallet
owner check succeeded; the callback result is the boundary.

A separate native uncertainty acknowledgement can permit a different independently
reviewed nonce. It does not permit another transaction at an already possible-send
nonce, retry/replacement, automatic cleanup or reuse of an approval. Public nonce
and hash tombstones survive transaction-history deletion. Restart never revives
native approval or a retired owner.

## Existing profiles and entry points

Migration preserves every legacy hash and its own candidate digest. Multiple
legacy hashes at one nonce create a single blocking slot, without guessing which
replacement won. Dispatched, acknowledged, terminal and legacy records without a
proven phase remain blocking. Reserved-only legacy workers are fenced by database
triggers before they can advance their old dispatch marker; known-unsent records
can subsequently be retired. Already possible legacy sends remain possible.

The schema requires exact owners for dispatch inserts and phase changes, and
rejects dispatch identity changes, conflicting monitoring records, hash rebinding
and retired-owner revival. These guards also apply to a connection opened before
the migration. Newer schema versions prevent an older binary reopening normally.
Opening a profile with missing nonce guards/index fails validation.

x402 retains its distinct random EIP-3009 authorisation-nonce and approval ledger;
it does not consume an EOA transaction nonce. No new signing route was added.
This coordination covers participants sharing one application database, rather
than external wallets or independently copied profiles. RPC freshness and native
approval cannot freeze inclusion-time state; their existing checks still apply.

Synthetic tests use temporary databases/app profiles, no real vault or live
submission. They exercise separate connections, spelling aliases, stale owners,
immutable hashes, storage faults, restart/acknowledgement, failed receipts/history
retention, legacy duplicates, old-worker SQL and RAII lifetime. Native interaction
and separately authorised live tests remain qualification gates.
