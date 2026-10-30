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
