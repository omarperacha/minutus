# Reviewed transfers from chat

Chat can request ETH and native USDC transfers on Base mainnet, Base Sepolia,
Ethereum mainnet and Ethereum Sepolia. This is the bounded Phase A capability,
not arbitrary contract execution. Explicit network, exact hexadecimal recipient
and positive decimal-string amount are required. Sender, USDC contract, chain,
nonce, calldata and fees come from trusted host state. ETH calldata is empty;
USDC uses the trusted native contract and exact ERC-20 transfer encoding.
ETH recipients must be code-free accounts: standard RPC cannot establish the
effects of a contract fallback, so an inconclusive contract transfer is rejected
before chat opens approval. Wallet retains its existing explicit warned approval
for contract recipients; this chat restriction does not remove that capability.

## Approval and lifecycle

The Wallet screen and chat share native candidate preparation, independently
decoded review, exact single-use approval, fresh app-password signing and
broadcast. The chat origin is host-authored and includes hashes of the
conversation and call identity; it is bound into the random approval session.
Optional model reason text never populates trusted facts or grants authority.

After password entry and again before submission, the host refreshes chain,
pending nonce, balances, gas, fees and exact-block simulation. Changed nonce,
reduced balances or increased fee/gas bounds require a new candidate and native
review. Chat requires a successful simulation. Wallet retains the warning policy
explicitly reviewed by the user: an unchanged inconclusive/unavailable/reverted
warning or improvement to success is allowed; a changed or mismatching warning
requires a new review. A lower fee estimate does not mutate
the approved signed payload. These checks cannot guarantee future chain state.
Account selection, vault revision, lock, sleep, expiry and cancellation invalidate
pending work. Signing and final transport admission retain the wallet epoch.
The actual request cancellation token is owned through signing and final send
admission and checked synchronously. A watcher dismisses native UI; its polling
interval cannot grant signing or submission authority.

Chat reviews have a two-minute lifetime and cancellation closes native review or
password input. At most one transfer attempt is admitted per persisted user turn,
including refusal, cancellation and preparation failure. SQLite schema 20 retains
identity-only turn tombstones across conversation saves and process restarts;
conversation deletion removes them. They are replay guards, never approval or
signing authority. Parallel calls cannot bypass the unique claim. No tool result
contains a password, secret key, raw signature, signed transaction or reusable
approval handle.

SQLite schema 21 adds a reservation/dispatch journal while preserving schema-20
turn claims. Each reservation belongs to its exact host review attempt. A worker
must atomically advance that exact reservation before any send; retiring an unsent
reservation prevents a paused worker from sending or deleting a newer attempt.
Both Wallet and chat can retire proven-unsent reservations and prepare afresh.
A crash after the dispatch marker is conservatively uncertain even if the socket
send may not have started. Storage failure never permits a send.

The local transaction hash and public monitoring metadata are retained before
final send admission. Transport failure, mismatched RPC hash or interruption
after admission is uncertain; no automatic rebroadcast occurs. Pending records
still need checking in Wallet. If they remain unresolved, a new chat transfer
request opens a separate native uncertainty review, defaulting to Keep blocked.
It shows the exact earlier hash/account/network/nonce and warns that allowing
new requests can cause additional payments. An explicit native acknowledgement
stops that old hash from blocking future chat requests; it does not claim the
old transaction failed, cancel or resend it, or approve spending. Its monitoring
history remains pending until chain evidence resolves it. Every new transfer
still requires its own native transaction approval and fresh password.
A missing hash never by itself authorises retry or acknowledgement.

Chat makes one read-only status refresh after node acceptance and reports
confirmed or reverted only when the existing chain monitor establishes it.
Otherwise the result remains submitted/pending; later confirmation, revert and
replacement are available in Wallet. A signature or response text proves none
of these chain outcomes.

## Qualification

Automated checks use synthetic intent and wallet fixtures, not real funds.
Native chat review/password interaction, lock/sleep/cancel races and complete
model trajectories require packaged manual qualification before a release claim.
No live testnet or mainnet transfer is authorised merely by implementing this
feature. Existing native Wallet evidence does not by itself qualify the new chat
entry point. Record packaged native checks separately from automated results
when qualifying a release.
