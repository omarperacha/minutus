# Contributing to Minutus

Keep changes focused on one observable outcome and include the tests needed to
verify it. Read [architecture](docs/architecture.md), the [crate boundaries](crates/README.md)
and [engineering conventions](docs/engineering-house-style.md) before changing
privileged behaviour or dependencies. For model-facing tool, context or controller
changes, also read [agent and tool design](docs/agent-design.md).

Write documentation and user-facing prose in British English. Preserve exact
commands, API identifiers, file paths, official names, licence texts and quoted
source material.

Select Node.js 22.23.4 (`nvm use` from the checkout) and pnpm 10.2.0.
After [bootstrap](docs/build.md), run the following checks sequentially:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked -- --test-threads=1
sh scripts/test-computation-xpc-containment.sh
pnpm check
pnpm build
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_dependency_policy.py
pnpm audit --audit-level moderate
git diff --check
```

Use focused tests during development. Some model, hardware and live-network tests
are intentionally ignored and require their documented fixtures. Report skipped
checks and existing failures rather than weakening lints or security policy.
The default desktop configuration excludes the old prototype feature; also run
`cargo test -p zeta-desktop --no-default-features --locked` for wallet/host changes.
Changes to x402 additionally need release-mode checks, so debug-only adapters
cannot mask a missing production path:

```sh
cargo test -p x402-client --release --locked -- --test-threads=1
cargo test -p zeta-desktop --release --test x402_release_surface --no-default-features --locked -- --test-threads=1
```

Changes to native worker limits or supervision additionally need
`scripts/test-computation-sandbox-host.sh` with the relevant `normal`, `cpu`,
`memory`, `wall` or `stress` fixture. These launch synthetic signed test bundles.

For bugs, include the commit/version, macOS version, Mac chip/memory, model,
reproduction steps and a redacted error. Check existing issues first. Source-beta
updates are rebuilt locally; do not include generated native files or weights.

Update the relevant architecture or security document when changing a trust
boundary. Keep privileged authority out of IPC values and model-visible tools.
Never attach wallet seeds, private keys, user conversations, credentials,
provisioning profiles or raw private diagnostics to an issue or pull request.

## Issues and pull requests

Use the bug-report form for defects and the feature-request form for proposed
improvements. A useful feature request explains the problem and desired outcome;
an implementation plan is optional. Security vulnerabilities belong in private
reports through [SECURITY.md](SECURITY.md).

Open ordinary pull requests against `dev`. `main` contains the current release;
only release promotions and fixes for that release target it. See the
[branching workflow](docs/branching.md). Describe the user-visible change, include
reproduction steps where relevant, and list the checks you ran or skipped.
