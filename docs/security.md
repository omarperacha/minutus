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
requests. That acknowledgement retains monitoring and grants no signing authority or
permission to reuse a possible-send nonce. Wallet/chat share a durable
[nonce owner](evm-nonce-ownership.md) before fresh authentication.
See [chat transfer boundaries](chat-transfers.md) for
freshness rules, cancellation and qualification limits.

### Source-qualified contract workflows

Qualification builds passing packaged compiler and isolated-worker admission can
separately advertise [version 2 declared contract workflows](generic-contract-qualification.md).
Release availability remains disabled pending native interaction qualification
and final human security review. Packaging pins alone cannot promote this
capability. The earlier bundled-profile API remains compatible.

Version 2 binds arbitrary ABI declarations to locally reproduced source and
proof-checked current runtime/dispatch. A pinned official Solidity compiler runs
in a separate bounded helper. Supported compiler versions and AST features are
restricted: complete typed snapshot effects must also have an independently
checked bounded source-field transition envelope. Unsupported state-dependent
effects, opaque external calls and mutable proxy dispatch reject. Supported
high-level calls require a complete source-derived graph within the explicitly
approved business scope, owner-qualified disjoint effects and exact replay frame
agreement. Compiler-bound immutable values do not imply constructor invariants.
Historical compilers have a separate restricted scalar feature tier based on
their original settings and reviewed bug triggers. This is a
generic analysed subset, not support for every EVM function. Source labels,
function names, logs and return values do not establish token, authority or
business semantics. The host does not offer a warning override for missing proof.

Each scope requires native outbound consent before RPC or public-source/compiler
acquisition. Every write requires independent exact native review and a fresh
password. Endpoint, account, ordered declarations and qualified source identities
are bound throughout the request. Historical completion evidence is distinct
from current prerequisites, so later actions may consume an earlier value without
erasing its history. Dependencies require declared completion conditions; final
goals are checked separately. A successful receipt without declared final goals
reports execution confirmed, not business-goal success. Observations describe
their proved blocks, not finality, later state or transaction causation.

The durable journal is wired to the native controller and shares Wallet/chat
turn tombstones and nonce ownership. Its final workflow and nonce admission
commit atomically. Stored history cannot reconstruct fresh approval or proof
capabilities. Restart, cancellation, ambiguous send or reorganisation never
authorise automatic resume, nonce reuse, cleanup or retry. Earlier allowance or
other authority may remain after a later step fails. Account upgrades and
arbitrary message/typed-data signing remain unsupported.

Before the first write, the host also bounds the complete historical, goal and
residual report against its 64 KiB output reservation. This includes actual
declared paths and worst-case typed values and JSON escaping. Oversized workflows
reject before submission; evidence is not truncated after a send.

### Unsigned contract observations

[Contract observations](contract-observations.md) use configured host-owned RPC
endpoints and fresh native outbound consent for exact public target/caller/call
bytes. They never access a wallet or signer. Calls are zero-value simulations at
canonical hash-pinned state with strict bounded ABI decoding with resource preflight and canonical re-encoding; declared ABI
and provider data are untrusted. Canonicality is rechecked, never treated as
finality. Latest reads bypass request and batch reuse. Cancellation gates each
RPC dispatch. Missing methods/state, wrong chains and unknown ABI layouts fail
without weaker proof. Optional `includeSource` inspection separately discloses
public source/compiler acquisition and returns bounded locally reproduced ABI,
layout and source identities without comments or documentation. These unsigned
snapshot observations grant no signing authority. Runtime identity or eth_call
success alone cannot qualify a workflow's effects or inclusion-time safety.

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


### Execution trust

The [local EVM workers](local-evm-simulation.md) run pinned upstream engines in
mandatory macOS sandboxes. The host binds acquired account/storage proofs and
code to one header/state root; provider chain identity remains trusted. Worker
JSON is untrusted discovery data and cannot mint a qualified candidate. Generic
qualification independently replays the frozen proved state with maintained
Revm in the host. That replay engine and the pinned compiler are explicit trusted
components; helper image attestation alone does not prove their correctness.

Base material preview excludes fee-sensitive application behaviour and labels
native balances as conservative maximum prepayment rather than exact settlement.
Exact signed Base execution then runs in the sealed offline handler over frozen
state. The signed bytes cannot direct new online acquisition, and post-input
worker failures collapse to one rejection without child-controlled diagnostics.
These checks are integrated into the qualification-only native workflow; their
automated tests do not replace the remaining native interaction and release
security gates.
