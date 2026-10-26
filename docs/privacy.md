# Privacy and network access

Chat inference, image understanding and speech recognition run locally.
Conversations, settings, managed images, models and wallet vaults are stored on
the Mac. Granted workspace operations inspect or modify local files through
host-enforced permissions. A local model does not make every app feature offline.

Network access occurs for distinct features:

- Model installation fetches catalogue-pinned artefacts from the named model host.
- Web search uses a configured Brave Search credential; retrieval contacts the
  requested public website. Queries, URLs and requested content reach those services.
  Native retrieval review displays the complete URL, including its path and query;
  these fields can carry private data even when a request has no body.
- EVM balance queries, simulations and transaction broadcasting use configured RPC
  endpoints. Public account addresses and transaction data can reach those endpoints.
  Opening an unlocked wallet can query balances and activity automatically;
  signing and payments still require approval. Public defaults are used when no
  custom endpoint exists. A custom endpoint that cannot be read or validated stops the operation instead of silently using a default.
- Public x402 requests contact the selected HTTPS service. Native review displays
  the exact URL and any JSON body before the unpaid request; a separate approval
  is required for the Base mainnet USDC payment. The service receives the
  approved payment signature on one identical paid retry. The localhost/Base
  Sepolia qualification transport remains debug-only. See [x402 support](x402.md).
- The manual update check contacts `minutus.ai` when an updater public key is
  configured. Installer downloads require successful signature verification.

Retrieved content can become local conversation evidence. A workspace write or
explicit export can also place data in a user-selected location. Providers and
external applications have their own privacy policies.

## Local storage and deletion

Ordinary conversations, tool history, settings and managed images are not
encrypted by Minutus. Wallet vaults are encrypted using the app password; wallet
credentials and recovery material stay in native windows. Brave API keys and
custom RPC settings use separate Keychain storage. FileVault, account access and
backups remain operating-system responsibilities.

Deleting a conversation removes its active database records and reaps images
that are no longer referenced. It is not secure erasure. Text may remain in
SQLite database pages or the write-ahead log, development traces, filesystem
snapshots and backups. Deleting a conversation does not clear those traces or
external copies. Minutus does not promise forensic removal or provide an
encrypted-conversation mode.

## Models and diagnostics

The application bundle contains native runtimes, not model weights. Chat and
speech models are separate installations. The catalogue supplies sizes, hashes,
requirements and licence references. Cancellation and integrity validation are
part of the download manager; incomplete downloads are not admitted as installed
models. Model removal is an explicit operation.

Supported chat weights can also be registered from local files. This action
reads and hashes the selected files without downloading, copying or moving them.
Their locations and verified identities are retained in the local application
database. Keep external files at those locations or use **Locate local files**
in the expanded import controls to re-register them. **Remove from Minutus** forgets the registration and removes
app-owned runtime references; it does not delete external weights. Local paths
are not supplied to the chat model or returned through the file-selection IPC.
SHA-256 is checked when importing or locating files again. Later availability
checks use access, file size and modification time; Minutus does not repeatedly
hash weights on startup or before responses. These checks do not guarantee
detection of deliberate local edits that preserve those attributes.

Operational audit records use bounded, content-free values. The README's default
source-running path uses a release build, which omits raw debug tool tracing.
Debug builds, including `pnpm tauri:dev` and the default `pnpm tauri:stage`,
automatically retain raw tool arguments and results under the app's
`dev-diagnostics` directory. These can include workspace text, paths and sensitive URLs. They are separate from ordinary
diagnostic logging and are not disabled by its preference or included in
diagnostic exports. The raw trace rotates at 4 MiB and retains one previous file;
conversation deletion does not remove either file. Execution/cache telemetry is
also retained locally for development. These traces are local files rather than
a remote analytics feed. Treat locally retained traces as private.
Do not attach raw logs, conversations or wallet material to public reports.
