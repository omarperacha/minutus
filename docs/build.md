# Building Minutus

Build on an Apple Silicon Mac running macOS 13.3 or later. The commands below
produce the supported Apple Silicon desktop app.

## Node.js build toolchain

Use **Node.js 22.23.4** and **pnpm 10.2.0**. The repository's `.nvmrc` pins Node;
CI reads the same file. `package.json` and `.npmrc` reject other Node versions.
Node runs the frontend development server, build tools and tests; it is not
included in the packaged desktop app.

If you use nvm, run these commands from the checkout before installing dependencies:

```sh
nvm install
nvm use
npm install --global pnpm@10.2.0
node --version  # v22.23.4
pnpm --version  # 10.2.0
```

Otherwise install Node.js 22.23.4 with your preferred version manager. Existing
terminal sessions and running development servers may retain an older Node;
select the pinned version and restart those servers. Do not loosen the engine
check to bypass a version mismatch.

## Run the release build

Install the prerequisites listed in the root README, then run from the checkout:

```sh
./scripts/setup-release.sh
open target/aarch64-apple-darwin/release/bundle/macos/Minutus.app
```

The script checks Apple Silicon, the Xcode command-line tools, Python 3.12+,
CMake 3.24+, Rust tooling and the pinned Node/pnpm versions. If nvm is installed
under `NVM_DIR` or `~/.nvm`, it installs/selects the pinned Node version. pnpm
and system prerequisites must already be installed. It then runs, in order:
`pnpm install --frozen-lockfile`, `python3 scripts/bootstrap.py`, `pnpm check`
and `pnpm release:build`. Any failed step stops the script. Launch remains a
separate command; the script does not open the app or modify its user data.

You can still run these steps individually for troubleshooting or pass
`--jobs` directly to `scripts/bootstrap.py` to change its default of four jobs.
This limit also applies to the tokeniser build; CI requests three jobs.
Direct `sh scripts/build-llama-sidecar.sh` builds default to three jobs, or accept
a job count from 1 to 16 as their first argument.

Bootstrap authenticates the public accelerated Qwen source archive using
`scripts/public-runtime.lock.json`. That source authenticates its upstream
archives and patch. The tokeniser uses an exact llama.cpp commit; the Whisper
helper uses the source hash in `scripts/asr/models.lock.json`. Sources and native
outputs are cached locally and excluded from Git. Runtime archives are sizeable;
bootstrap does not download speech or chat weights.

Native outputs are local to each checkout and are not copied by source syncs.
If a build reports a missing computation runner, tokeniser or speech helper,
run `python3 scripts/bootstrap.py --jobs 4` before retrying `pnpm release:build`.
If only the computation runner needs rebuilding, run
`sh scripts/build-computation-runner.sh` from the repository root.

The public Qwen patch matches the retained application patch. Compiler versions,
build paths and source packaging can change binary hashes. Bootstrap therefore
records a local build pin in `.cache/minutus-source/qwen-built.lock.json` and
provenance in `source-build.json`; it never replaces the committed official
binary pin. Ordinary staging still authenticates each generated payload.

An interrupted Qwen build may leave incomplete work directories. Preserve any
useful logs, then use a new checkout or remove that incomplete cache before
retrying. A changed cached archive is rejected rather than silently trusted.

## Local packaging and wallets

Local packaging seals the app and nested helpers ad hoc. The macOS wallet uses
an encrypted Rust app vault protected by an app password, so source builds need
no Apple developer account, signing certificate, team or provisioning profile.
Rebuilding and resealing the same application preserves access to the existing
password vault. Official distribution signing and notarisation remain separate.

Existing legacy vaults are not silently migrated; retain their original recovery
material. See [wallet recovery](usage.md#wallet) before replacing an existing vault.

The product name is Minutus. Technical identifiers containing `zeta` remain to
preserve wallet identity, storage compatibility and native protocol contracts.
Do not change them as a cosmetic rename.

## Development runner and local traces

After installing dependencies and bootstrapping the native inputs, the optional
development runner enables frontend hot reload and Rust debug builds:

```sh
pnpm tauri:dev
```

Debug builds automatically retain raw tool arguments/results in
`dev-diagnostics/zeta-tool-trace.jsonl` beneath the app data directory, with
separate local execution/cache telemetry. Traces can contain workspace text,
paths and sensitive URLs. Raw tool tracing is independent of the ordinary logging preference and diagnostic
exports. Keep development inputs synthetic and do not upload raw traces. Release
builds omit this tracing; ordinary diagnostics and transaction audit records are
separate.
See [privacy and network access](privacy.md#models-and-diagnostics).

`pnpm tauri:stage` instead builds an embedded-frontend debug app with ad hoc seals.
Its app is `target/debug/bundle/macos/Minutus Staging.app`, with identifier
`com.omarperacha.zeta.staging` and a separate app-data directory. Hot reload uses
`com.flowzed.zeta`; release builds retain `com.omarperacha.zeta`. The checkout
location does not create another profile: two builds of the same identity share
local data. Staging has the same sensitive debug tracing. Use `pnpm release:build` for the optimised
local package shown above.

Run the checks in [CONTRIBUTING.md](../CONTRIBUTING.md) before submitting changes.

## Moving data from older staging builds

Older `tauri:stage` builds shared the release identity and local data. Updating
source does not move that data automatically. Stop using the old debug
`Minutus.app`; the new debug bundle is **Minutus Staging.app**.

To assign the formerly shared data to staging, build the new staging app, open
it once to initialise its macOS container, then quit every Minutus/development
instance. In an authorised Terminal, run from the checkout:

```sh
python3 scripts/migrate-staging-data.py --archive-existing-staging
# Review the printed source/destination paths, then:
python3 scripts/migrate-staging-data.py --archive-existing-staging --apply
```

If macOS denies container access, grant **Terminal** Full Disk Access in System
Settings → Privacy & Security, then retry. Do not use `sudo`. The command moves
both existing sandboxed and pre-sandbox profiles to their corresponding staging
locations. It requires the new container to exist; it does not manufacture macOS
container metadata. It preserves any existing staging profile beside its new
location and writes private progress records/database snapshots under
`~/Library/Application Support/Minutus Staging Migrations`.

Moves use same-filesystem renames: managed weights and the encrypted vault are
not copied or decrypted. Known managed model paths are rebased; external weight
files stay where they are. Recovery material and the vault password are unchanged.
Project-folder permissions and external model access may need granting again;
use **Locate files** where necessary. Keychain credentials and OS permission
records are not migrated. Production creates a fresh profile after the move.
If migration stops part-way through, retain its record and snapshots and inspect
them before retrying; the script does not delete either profile or automatically
reverse a partial move.
