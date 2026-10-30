# Minutus Rust workspace

This directory contains the Rust business logic and privileged boundaries for
Zeta. The Tauri desktop host in `src-tauri/` provides native composition and
presentation.

## Dependency direction

~~~text
app-core (composition root)
├── agent-core
├── computation-runtime
├── audio-runtime (portable microphone capture)
├── app-storage ────────────> ipc-types
├── model-runtime  (chat inference and independent ASR model storage)
├── permissions <── filesystem-tools
├── audit-log
├── wallet-core ────────────┐
├── transaction-policy <────┴── x402-client
├── chain-evm <──────────── x402-client
│   └── evm-simulation (isolated local execution)
├── web-retrieval
└── ipc-types
~~~

The diagram describes composition, not authority transfer. The manifests are
the enforceable dependency graph:

- "app-core" is the Rust composition root and may depend on all subsystems.
- "app-storage" owns SQLite, migrations and ordinary local records. It is
  reachable only through Rust composition and has no frontend or vault API.
- "ipc-types" is a dependency-free domain leaf apart from serialisation and
  binding generation; it defines frontend-safe values but no capabilities.
- "filesystem-tools" depends only on capability decisions from "permissions".
- "x402-client" may construct EVM payment data using "chain-evm" and apply
  rules from "transaction-policy"; it cannot depend on "wallet-core".
- "chain-evm" may depend on "evm-simulation" for authority-free local execution
  values and a developer worker API. It owns the concrete RPC/proof adapter.
- "evm-simulation" owns the fixed process protocol, pinned upstream execution
  adapters and one-shot isolated lifecycle. It cannot depend on wallet,
  filesystem grants, permissions, application storage, HTTP clients, desktop UI
  or concrete agent orchestration. Its narrow private native resource FFI is
  documented in its README; no model tool or signing route selects the worker.
- All other crates begin as independent leaves.
- "agent-core" defines model-neutral orchestration. It cannot depend on
  concrete filesystem, wallet, chain, x402, networking or audit
  implementations.
- "computation-runtime" owns the fixed sandbox protocol, embedded interpreter
  policy and one-shot child lifecycle. It cannot depend on project storage,
  filesystem capabilities, the desktop UI or concrete agent orchestration.
- "web-retrieval" owns canonical public HTTPS targets, validated DNS connection
  plans and the bounded static-response broker. It cannot depend on agent
  orchestration, the desktop UI, browser state, project storage, model runtime,
  computation, wallet or x402 code.
- "audio-runtime" is a portable leaf for microphone device callbacks, bounded
  buffering, sample normalisation and level measurements. It cannot depend on
  the desktop UI, application sessions, model inference or IPC schemas.
- "desktop-lifecycle" is a non-secret macOS notification leaf used by the
  native desktop wallet composition. It depends on native bindings and
  block ownership, never on wallet, IPC, model or application crates. Its private
  Objective-C bridge is the documented unsafe-code exception in its README.
- "wallet-core" owns vault and signing behaviour. It may consume the non-secret,
  lifetime-bound `ConsumedApproval` from "transaction-policy" so signing cannot
  bypass exact approval verification and durable single-use finalisation. It
  cannot depend on model, agent, filesystem, chain transport or x402 code.
- Frontend/Tauri code must not be added to any privileged crate.

Cross-boundary communication will use narrow value types and traits. A future
dependency that reverses one of these arrows requires an architecture review.

## Crate responsibilities

| Crate | Responsibility | Explicitly excludes |
| --- | --- | --- |
| "app-core" | Application composition and use-case coordination | Frontend rendering and secret handling |
| "app-storage" | Versioned SQLite persistence for settings, conversations, grants and non-secret metadata | Frontend database access and wallet secrets |
| "model-runtime" | Local inference lifecycle and normalised generation events | Agent policy, workspace access and wallets |
| "audio-runtime" | Portable, bounded microphone capture and 16 kHz mono normalisation | Desktop permission presentation and lifecycle, transcription and frontend IPC |
| "desktop-lifecycle" | Non-secret macOS sleep, screen/session and app deactivation notifications | Secrets, wallet policy, IPC, signing and model authority |
| "agent-core" | Model-neutral reasoning/tool state machine and ports | Concrete privileged implementations |
| "computation-runtime" | Fixed computation contract, private runner protocol and isolated interpreter lifecycle | Project path resolution, shell execution, networking and desktop orchestration |
| "web-retrieval" | Canonical public HTTPS target validation, DNS-to-connection binding and bounded static retrieval | Browser automation, ambient credentials, model policy, payments and desktop orchestration |
| "permissions" | Capability grants and authorisation decisions | Performing filesystem, network or signing effects |
| "filesystem-tools" | Handle-relative operations inside active grants | Wallet, inference and network access |
| "wallet-core" | Encrypted vault, key lifecycle and signing boundary | Broadcasting, RPC, agent and workspace access |
| "evm-simulation" | Bounded local Revm/Base worker, fixed protocol and isolated one-shot lifecycle | Wallet secrets, signing, submission, network IO, project grants and effect/source qualification |
| "chain-evm" | EVM types, construction, decoding and simulation interfaces | Private-key storage and UI approval |
| "transaction-policy" | Candidate binding, approval lifecycle and transaction rules | Key access and broadcasting |
| "x402-client" | Bounded x402 v2 HTTP protocol and payment construction | Direct signing and autonomous payment |
| "audit-log" | Separate bounded streams for content-free operational diagnostics and public transaction audit events, with explicit previewed exports | Arbitrary messages, telemetry, prompts, image bytes, signing payloads, signatures and secret-bearing diagnostics |
| "asset-store" | Bounded image validation, normalisation and private managed files | Source paths, URLs, model policy and database ownership |
| "ipc-types" | Rust-owned frontend-safe IPC schemas and validation limits | Secrets, capabilities and privileged implementations |

### `app-core` module layout

`app-core/src/lib.rs` is the thin composition facade: it declares the private
feature modules, retains the long-lived `AppCore` service container and startup
wiring, and re-exports the established crate API. Application behaviour is owned
by focused modules rather than accumulated in the facade:

- `execution_context/` owns host execution state and its model-visible projection;
- `chat_generation/` owns prompt admission, runtime generation and shared prompt
  conversion and scheduling support;
- `computation/` owns model-facing computation contracts, granted project-input
  binding and verified result preparation;
- `web/` owns permissioned search, retrieval authorisation, result preparation
  and managed remote-asset handoff;
- `workspace/` separates project/grant coordination from privileged filesystem
  operations, their audit records and operation concurrency;
- `dictation/` owns optional speech setup and subsequent session coordination;
- `models/`, `conversations/`, `diagnostics/` and `transaction_audit` own model
  lifecycle, conversation/asset lifecycle, bounded operational records and
  public transaction-audit coordination respectively;
- `evm_transactions/` separates durable nonce-owner lifetime, exact dispatch
  fencing, public monitoring and chat-turn admission;
- `working_context/` owns bounded persisted-history projection and semantic
  checkpoint reuse;
- `projection` owns IPC-safe value conversion and `error` owns the application
  error surface and source conversions; and
- `tests.rs` holds crate-facade and cross-module behaviour tests, while focused
  feature tests remain beside their owning modules.

These modules may add inherent `AppCore` implementations without changing the
public facade. New work belongs with the responsibility it changes; it must not be
added to `lib.rs` merely because the composition root can access every subsystem.

## Workspace policy

- The minimum supported Rust toolchain is 1.97. Zeta pins Rust 1.97.1 in
  `rust-toolchain.toml` for reproducible local and CI builds.
- Versions, edition, Rust version, licence and shared dependencies are declared
  in the root Cargo.toml.
- Crates inherit workspace lints and must declare internal dependencies through
  the workspace dependency table.
- Third-party dependencies must use compatible versions in the workspace
  table; wildcard versions and unreviewed Git sources are forbidden.
- "cargo deny check" is the dependency advisory, source and licence gate.
- Every crate is "publish = false" until a separate publication decision.
- Production unsafe code is denied by default. A future native bridge must be
  isolated in a reviewed crate with documented safety invariants.

### Password-vault ownership

`wallet-core/password_vault/` owns bounded credential/KDF ownership, the
version-2 encrypted wrapper, shared private Unix persistence and cancellable work
identity. `wallet-core/app_vault/` owns the version-3 shared app catalogue and
fresh signing capabilities. The normal desktop wallet controller composes these
with native secret input; no Keychain or Apple developer identity is needed.
The non-default `password-vault-prototype` desktop feature remains an isolated
regression harness. `src-tauri/src/password_wallet/` owns native
presentation and lifecycle composition in the matching non-default desktop
feature. The `app-core` feature only forwards this capability for its synthetic
integration test; it adds no secret storage or model-facing API. See [wallet security](../docs/security.md) for the normal format, trust model
and qualification limits. The v2 prototype
record remains historical evidence.

### Imported model file access

`app-core/models/external/` owns verified file registrations and their lifecycle.
`app-storage` persists optional opaque native bookmark bytes with each reference;
old path-only records remain readable. The desktop implements `ExternalModelAccess`
with read-only macOS bookmarks and RAII scope ownership. Portable crates do not
depend on Foundation or native UI. Availability and verified use restore access
before filesystem checks; grants never replace SHA verification. Removal and
replacement stop inference before releasing scopes, and scopes outlive provider
teardown. No bookmark, path or native scope is exposed through model/frontend IPC.

### Speech model boundary

`model-runtime/catalogue/speech-v1.json` and `SpeechProfile` are an independently
versioned, ASR-only catalogue using the shared validated artefact identity.
`ModelCatalogue` remains chat-only. `model-runtime/asr/` owns compatible Whisper
artefact validation and a verified-use lease which excludes concurrent removal.
Its private speech storage reuses `ModelDownloadManager`; it introduces no chat
selection, microphone, platform audio or UI dependency. `app-core/dictation/`
coordinates installation, recoverable progress snapshots and one active dictation
session. Its session coordinator binds project, conversation, composer and draft
revision identity; owns ordered bounded events; and composes a verified speech-model
lease, portable capture and final-only recognition. Shared foreground admission
excludes dictation and chat generation without coupling their model lifecycles.
`ipc-types` exposes only bounded setup and session values. `audio-runtime` remains
an independent leaf: it exposes portable capture values and backend traits while
CPAL and platform stream types stay private. Desktop code owns permission copy,
bundle metadata, lifecycle cancellation and thin command adaptation.
