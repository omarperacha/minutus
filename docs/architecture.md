# Minutus architecture

The React frontend calls a closed Tauri IPC interface. Rust owns privileged
state and validates inputs before effects. The authoritative dependency graph
and subsystem responsibilities are in [the crate guide](../crates/README.md).

- `agent-core` owns model-neutral orchestration and tool contracts.
- `app-core` coordinates use cases without granting authority through model output.
- `permissions` and `filesystem-tools` mediate granted workspace access.
- `computation-runtime` uses a bounded native XPC service and isolated runner.
- `model-runtime` verifies model artefacts and manages local inference.
- `wallet-core` owns the password-encrypted app vault and signing;
  `transaction-policy` binds exact approvals.
- `desktop-lifecycle` supplies non-secret macOS lifecycle notifications to the host.
- `web-retrieval` owns bounded retrieval and public destination validation.
- `ipc-types` contains frontend-safe values, without keys or host capabilities.

The desktop host owns native review dialogues, application lifecycle and sandbox
composition. Sidecars are independent executable payloads, not model-granted
shell access. Release packaging retains the stock runtime and the independently
pinned accelerated Qwen backend. Runtime selection and rollback remain host policy.

The public repository contains application source, meaningful tests, approved
build inputs and public documentation. Internal experiments and raw diagnostic
runs remain outside the source distribution. Official release metadata identifies
the public source commit from which the app was built.

Agent/controller changes follow the [agent and tool design reference](agent-design.md).
General contributor conventions are in [engineering house style](engineering-house-style.md).

## Chat transfer composition

`chat_tools/wallet_transfer/` owns the closed model intent and public result.
`evm_transfers/preparation` owns shared Wallet/chat candidate preparation and
freshness checks. `evm_approval/registry` owns immutable pending reviews;
`evm_approval/execution` composes native review, signing and durable broadcast
admission. Wallet secret ownership remains in `wallet_lifecycle`/`wallet-core`;
chain validation and transaction encoding remain in `chain-evm`. SQLite schemas
20 and 21 add identity-only per-user-turn replay tombstones and public dispatch
facts respectively. The actual cancellation token crosses the effect path by
shared ownership; native UI polling is not the authority check. Native uncertainty
acknowledgement is separate from exact transaction approval and never signs. See
[chat transfers](chat-transfers.md) for the current security contract.
