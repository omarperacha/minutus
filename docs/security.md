# Minutus security boundaries

Model output is untrusted. Tool names, arguments, permissions, workspace scope,
operation limits and native transaction approvals are enforced by the host.
A plan or model-generated confirmation does not grant access or authorise payment.

The UI receives narrow IPC values. Vault keys, signing authority and approval
commitments remain inside Rust and native review boundaries. Computation uses a
bounded isolated runner; web retrieval validates destinations independently of
page content. Model weights are hash-verified when downloaded, imported or
located again; later availability checks use access, size and modification time.
Native helper payloads have separate integrity checks. See
[model verification](privacy.md#models-and-diagnostics).

## Workspace and outbound request controls

New folder grants are read-only. Enabling read and write authorises ordinary
project edits without a prompt for each edit; completed changes have diffs and
explicit undo. Review changes before running code, installing dependencies or
opening a project in tools that execute its configuration.

The filesystem broker rejects known credential files and protected configuration
paths for reads, discovery and writes, including when a protected directory or
its child is selected as the root. Matching is case-insensitive. Protected paths
include `.env` and `.env.*` except the exact `.env.example`, `.env.sample` and
`.env.template` names; private-key/certificate containers; Git/Hg/SVN metadata;
`.github`, `.vscode`, `.idea`, `.devcontainer`, `.husky`, `.cargo`, `.config`;
SSH, cloud, GPG, Kubernetes and Docker configuration; common credential files
and shell startup files. The exact policy is in
`crates/filesystem-tools/src/path_policy.rs`. There is no model-controlled override.
This credential list is a filename policy, not content-based secret detection: ordinary files
and example templates can still contain secrets, and source edits can still
become executable when another tool runs them. Grant narrowly scoped folders. The private app-data exclusion additionally uses
directory identities; selecting home or another root containing that data is
rejected. See [folder setup](usage.md#projects-and-files).

Every web retrieval requires native review of its complete canonical URL,
including the path and query, in a selectable, scrollable field. Deny is the
default action. Percent escapes remain encoded to preserve the wire spelling.
The website receives these values; approve only requests whose destination and
outbound data you understand. Diagnostic summaries remain redacted separately.
Search has its own approval displaying the query sent to Brave.
Markdown responses are admitted as bounded untrusted text with source provenance;
embedded HTML, scripts and links are not executed or fetched automatically.

Custom RPC configuration is stored in Keychain. A public default is selected
only when the custom item is absent. Denied access, unavailable Keychain and
invalid stored configuration stop the operation without switching providers.

Public x402 purchases use the same public-address DNS validation and pinned
connection policy as retrieval. The dedicated adapter additionally supports
bounded JSON POST bodies, reviewed with the complete URL before transmission.
Payment approval binds the exact URL, method, body identity, Base mainnet USDC
contract, payer, recipient and amount. The signed request is submitted once;
ambiguous outcomes do not authorise another payment. The localhost/testnet
qualification transport is compiled only in debug builds. See [x402 scope and
qualification](x402.md).

## Local machine trust

Minutus assumes a trusted macOS account and operating system. Its Ollama server
uses an unauthenticated ephemeral loopback port; other local processes may reach
that API. Workspace checks do not provide isolation from malicious software
already running with the user's privileges. Hard links to files with names
outside a granted root are not rejected. See [privacy](privacy.md) for ordinary
storage, development traces and deletion limits.

## Wallet recovery and transaction approvals

Wallet creation and recovery use native prompts for a 24-word recovery phrase.
Store the phrase securely offline. Losing it can prevent recovery; disclosing it
can give another person control of the wallet. Never enter it into chat or a
public issue. Imported-key accounts have their own backup requirements.

The Rust app vault uses one app password for independent accounts. Each generated
or restored phrase and each imported private key needs its own recovery backup;
a phrase does not recover other independent accounts. Passwords, phrases and
private keys are entered through native windows, never chat or ordinary frontend
IPC. The wallet requires no Apple developer identity or protected Keychain item.
Brave API keys and custom RPC configuration retain their separate Keychain storage.

The password wraps a random vault key using Argon2id (256 MiB, six passes, one
lane); account payloads use authenticated XChaCha20-Poly1305 encryption. A stolen
vault permits offline password guessing. Old copies remain decryptable with their
old password, and encryption alone cannot prevent whole-file rollback. A
compromised operating system or same-user malicious software is outside this
boundary. Forgotten-password reset replaces the vault after explicit native
confirmation; it cannot decrypt existing accounts without their recovery material.
Legacy wallets are not silently migrated.

Private application data and overlapping workspace roots are excluded from model
filesystem and computation inputs. Receiving addresses are withheld until password
authentication; locked envelope metadata is not a trusted deposit destination.
Transactions require native review of the exact proposal and fresh app-password
authentication. Approval is bound to the candidate and consumed once. Screen lock,
sleep and inactivity revoke the unlocked session. A network failure or ambiguous
broadcast does not authorise an unrestricted repeat payment.

## Software releases

Official macOS distribution requires hardened runtime signatures on nested native
code, notarisation, and a stapled installer tested through Gatekeeper. A local
ad-hoc build is not an official signed distribution.

Updates remain disabled until a release public key is compiled into the app.
Enabled updates verify the full installer and the signed version before saving;
installation uses the downloaded DMG through Finder. The offline signing key is
never part of this repository. See [release process](releases.md) and
[reporting vulnerabilities](../SECURITY.md).

## Contributor verification

The [contributor checks](../CONTRIBUTING.md) include signed XPC execution and
containment tests, plus Rust and JavaScript dependency policy. Packaged fixtures
are ignored by ordinary Cargo runs and must be exercised through their scripts.
A green unit-test run alone does not qualify the native sandbox.

## Reviewed chat transfers

Chat ETH/native USDC transfers use the same native approval and fresh-password
signing boundary as Wallet on the four supported Base/Ethereum networks.
Per-turn durable admission blocks automatic repeat attempts. Known transaction
hashes are retained before submission so uncertain outcomes can be reconciled
without rebroadcast. Proven-unsent reservations are retired atomically; uncertain
submissions require a separate native acknowledgement before further chat
requests. That acknowledgement retains monitoring and grants no signing authority.
See [chat transfer boundaries](chat-transfers.md) for
freshness rules, cancellation and qualification limits. Arbitrary contract calls
and arbitrary message/typed-data signing remain unavailable.

### Transfer fee budgets

Native ETH and USDC review distinguishes quoted fees from the retained
pre-submit budget. Base L1/operator quotes receive 50% headroom (rounded up in
wei) before approval; the budget is bound into exact approval evidence and is
never increased afterwards. Refresh retains the approved execution cap and tip,
checks current base-fee coverage, compares raw rollup quotes to the retained
budget and keeps nonce, balance, intent and simulation checks. Costs above the
budget require a new candidate and approval. No automatic repricing or retry is
introduced. Ethereum's Base-fee components remain zero.

The signed EIP-1559 fields cap execution pricing, not Base's separate L1/operator
charges at inclusion. The native review discloses that the total budget is checked
before submission and cannot guarantee those future charges.
