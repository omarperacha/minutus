# Native contract review qualification

Contract workflows are available only in a debug qualification build with both
packaged attested workers. Release builds keep the new write tool disabled until
native interaction qualification and final human security review are recorded.
There is no environment, model or warning override for that gate.

## Display-only preview

Build the normal debug binary, then run:

```sh
cargo build -p zeta-desktop --locked
target/debug/zeta-desktop --contract-approval-preview
```

This early debug entry point creates only the native application and the actual
production approval renderer with a fixed synthetic display. It runs before
application setup and cannot open profiles, databases or vaults, contact RPC,
issue approval commitments, request a password, sign or send. All three choices
only close the fixture. Release builds reject the argument without starting the
application. Do not enter any wallet password: this preview never requests one.

Check the following on the actual screen, including a small window/screen size:

- The title and body clearly identify the synthetic fixture.
- Deny is the default keyboard action; Return must not approve.
- Approval requires explicitly choosing the separate approval button.
- The complete text is selectable and scrollable, including the end marker.
- Decision buttons stay visible outside the text pane.
- Exact network/account/target/function/amount/allowance/fee details are legible.
- The allowance remaining after a later failure and Base fee limitations are clear.
- Approve, Deny and Dismiss each close only the fixture; no password or send follows.

The fixture checks rendering and native decision interaction. It does not
qualify an actual vault/password/signing workflow or prove a live transaction.

## Initial native unlock

An existing selected locked account opens the same native unlock prompt used by
chat transfers and x402, while the user remains in chat. The host consumes the
user turn before prompting. Cancelling unlock must stop before outbound scope
review, RPC/source acquisition, transaction approval or signing. A fresh user
message is required for another attempt.

Successful initial unlock allows account-bound scope review and qualification.
It is not transaction approval or signing authentication. After exact native
approval, every write still requests a separate fresh signing password.

## Standard Ethereum Sepolia write fixture

Use the public `Storage` contract below for the single-write native gate. This
is a test fixture selection, not an admission allowlist or a source of signing
authority. Every run still needs ordinary source, proof, effect and refresh
qualification.

| Field | Standard value |
| --- | --- |
| Network | Ethereum Sepolia, chain ID `11155111` |
| Contract | `0x2738d13e81e30bc615766a0410e7cf199fd59a83` |
| Write | `store(uint256)`, selector `0x6057361d` |
| Arguments | One decimal integer `N`; start with `42`, or `43` if the proved current value is already `42` |
| Native value | `0` ETH; Sepolia ETH is needed for the transaction fee |
| Read-back | `retrieve()`, selector `0x2e64cec1` |
| Declared final field | `number` equals `N` |

[Sourcify's public API example](https://docs.sourcify.dev/docs/api/) identifies
this deployment. Its [complete original compiler input and source](https://sourcify.dev/server/v2/contract/11155111/0x2738d13e81e30bc615766a0410e7cf199fd59a83?fields=all)
contain one `uint256 number` in slot zero. `store` assigns its argument to that
field; `retrieve` returns it. There are no events, token approvals, modifiers,
external calls or proxy dispatch. The compiler is official
`0.8.7+commit.e28d00a7`, with optimisation explicitly disabled. This exercises
the restricted historical scalar tier, not the broader call-graph features.

On 11 November 2026, a fresh read-only RPC check returned chain ID `11155111`,
the same 336-byte runtime as the published source record, and `retrieve() == 1`.
Runtime SHA-256 was
`7db4de12f59f8a8230629b96ddcb7a9405dc0b5aaf5878bdf89cde697503bac6`.
These observations do not replace local reproduction or canonical proof checks.
The current default PublicNode endpoint rejects exact-header proofs with error
4444, `distance to target block exceeds maximum proof window`. A read-only
replacement-endpoint probe on the same date obtained account and four storage
proof responses from `https://sepolia.gateway.tenderly.co`, exact-hash code, the
correct chain ID and a matching canonical-header recheck. This establishes
transport availability for the exercised requests, not complete candidate or
human-gate qualification. See [Tenderly's public endpoint reference](https://docs.tenderly.co/node-rpc/rpc-reference).

In Staging, open **Settings → EVM RPC connections → Ethereum Sepolia → Set custom
URL**, enter that HTTPS URL in the native prompt, and choose **Save securely**.
Start a fresh chat request after changing the endpoint. Normal proof verification
and all admission/resource limits still apply; an in-flight request retains its
original endpoint. The debug `--qualify-generic-contract` command separately
hardcodes the default RPC and does not load this setting, so changing Settings
does not change that CLI command's result. Do not use it to judge the custom
endpoint. A different contract does not repair the default provider failure.

In the isolated `pnpm tauri:stage` profile, using the deliberately disposable
test wallet, run the same request separately for each decision path:

1. Read the current field and choose `N` so the intended successful write changes
   it. Ask chat to call `store(uint256)` with `["42"]` (or `["43"]` when chosen),
   zero native value, and require `number == N` after confirmation.
2. Deny or dismiss native transaction approval. The workflow must stop without
   requesting a signing password, signing or submitting. An initially locked
   wallet may already have requested its separate unlock password.
3. Start a fresh request, approve native review, then cancel authentication. The
   workflow must stop without signing or submitting.
4. Start another fresh request, verify the exact network, account, address,
   method, argument, field relation and fee bound, then approve and authenticate.
   Require exactly one submission and canonical inclusion with a successful
   receipt matching the signed transaction. Require fresh proved source-field
   completion and final-goal conditions `number == N` at a named block.
   Independently read `retrieve()` at the receipt block as a cross-check; this
   read alone is not proof authority and observes end-of-block state, including
   later transactions in the same block. Every attempt uses fresh qualification;
   fresh signing authentication occurs only after explicit approval. Never resume a
   cancelled approval.

Anyone can overwrite this globally shared cell. A subsequent value change is
not evidence that cancellation submitted, nor a lasting guarantee from a prior
write. A freshly proved field value unequal to `N` makes the current condition
unsatisfied. Missing proof or unresolved canonicality makes it unknown, not
successful. Preserve the receipt separately in either case. Any repeat starts a
new request with a distinct `N` and a new explicit approval; never automatically
repair or retry. Retain the Staging build, account, chosen argument, decision
path, transaction hash, receipt block and observed field as the human test
evidence. Do not record passwords or private keys.

## Remaining human gates

Review the source/effect/protocol/nonce/workflow evidence and exact supported
coverage before promoting this capability. Unknown source/proxy/function
families remain unsupported even after the human gate is closed.

Any actual native password/signing qualification needs a separately isolated
Staging identity/profile, explicitly synthetic test keys and a deliberate test
plan. Verify per-step fresh authentication, account switching, lock/background,
cancel and crash/restart behavior, partial allowance and uncertain-send display.
Do not use an ordinary profile or real vault as an implicit fixture. A live funded
or testnet submission, real keys, release or deployment requires separate explicit
authorisation. No automated/subagent result is an external security audit.
