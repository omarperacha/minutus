# Temporary tokeniser dependency exception

The current dependency path `model-runtime → tokenizers 0.22.2 → paste 1.0.15`
has a temporary maintenance exception, approved on 13 October 2026. Review or
remove it before **13 October 2027**, or before changing that dependency path.
The policy gate rejects builds on or after the deadline.

`paste` is unmaintained ([RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)).
This advisory reports maintenance risk, rather than a specific vulnerability.
A version bump is not sufficient unless the resolved Rust dependency graph
actually removes `paste` and tokeniser behaviour remains compatible.

## Scope and enforcement

The approved dependency is a compile-time proc-macro introduced only through
`model-runtime → tokenizers 0.22.2 → paste 1.0.15`. Maintenance risk remains:
future defects may have no upstream fix. This exception does not certify code
security or waive any other vulnerability, source, licence or integrity check.

Run the complete policy gate with:

```sh
python3 scripts/check_dependency_policy.py
```

The wrapper checks the resolved all-feature graph for the exact versions,
proc-macro target and sole direct dependent before invoking `cargo deny --locked check`. It rejects
new users, version changes, removal of paste while its exception remains, and
execution on or after 2027-10-13 UTC. Public CI runs this wrapper.

Direct `cargo deny check` honours the named advisory exception but does not enforce
this deadline or path guard. Use the wrapper for release qualification. The date
is an enforced review deadline, not a scheduled reminder.

## Removing or renewing the exception

Prefer a stable tokeniser release that removes paste. Before adopting it, test
artefact loading, added and special tokens, truncation behaviour, Unicode and
whitespace boundaries, and pinned Ollama/Qwen token-count parity. Remove the
advisory exception and temporary guard when no longer needed. Renewing it needs a
new documented review; do not silently extend the deadline.

## Duplicate-version warnings

The reviewed 24 October 2026 baseline in `deny.toml` skips duplicate warnings for
exact older versions required by Tauri/platform bindings, build tooling and the
coexisting RustCrypto dependency generations. It does not skip advisory,
licence, source or yanked-version checks for those packages. New duplicate
versions still warn; remove obsolete baseline entries when dependencies change.
The policy runner hides repeated inclusion graphs to keep CI output readable.
Obsolete GTK advisory exceptions have been removed: the supported macOS graph
does not encounter them. Any future Linux support requires a fresh dependency
policy review. The remaining `proc-macro-error` exception concerns build tooling,
not GTK runtime access.

The lockfile includes `ctutils` 0.4.5 and Tokio 1.53.3 for RUSTSEC-2026-0385 and
RUSTSEC-2026-0386. These advisories are not ignored.


## Wallet cryptography updates, 24–26 October 2026

RUSTSEC-2026-0387 (published 23 October) affects the previously locked
`crypto-bigint` 0.5.5, inherited through `bip32` 0.5.3 → `k256` 0.13.4 →
`elliptic-curve` 0.13.8. This chain predated the model-import changes.

The stable upstream upgrade uses `bip32` 0.6.0, `k256` 0.14.0,
`elliptic-curve` 0.14.1 and patched `crypto-bigint` 0.7.9, with `ctutils` 0.4.5.
The initial 24 October update selected 0.7.6. Upstream subsequently yanked
0.7.6–0.7.8 following the `NonZero<T>` multiplication trait correction in
[RustCrypto #1373](https://github.com/RustCrypto/crypto-bigint/pull/1373).
The 26 October follow-up changes only the locked `crypto-bigint` version and
checksum to 0.7.9. Registry yank status is live metadata: an unchanged lockfile
can therefore begin failing dependency policy after previously passing.
The removed `std` feature of `bip32` is replaced by its `alloc` feature; the pure
Rust secp256k1 implementation remains selected. No advisory exception, crypto
backend replacement or wallet-format migration is introduced.

API adaptation preserves uncompressed SEC1 public keys, deterministic recoverable
signatures and explicit rejection of high-`s` signatures. Existing fixed address
and signature vectors, vault reopen/restore tests, approval expiry and durable
single-use checks qualify the integration. The duplicate baseline also covers
exact older HMAC, SHA-2 and RNG trait versions still required by other subsystems;
these entries affect duplicate warnings only, not vulnerability checks.
