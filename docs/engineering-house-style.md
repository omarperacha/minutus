# Minutus engineering house style

Status: active

This document records the default engineering choices for Minutus. It exists to
make routine design and review decisions consistent without turning conventions
into inflexible rules. Security and architecture requirements take precedence;
the workspace boundaries in [the Rust workspace guide](../crates/README.md) are
authoritative.

Write documentation and user-facing prose in British English. Preserve exact
commands, API identifiers, file paths, official names, licence texts and quoted
source material.

## Core principle: organise around responsibility

Each crate, module, type and function should have one coherent reason to change.
Group code that enforces the same invariant or implements the same feature
boundary. Separate code when it changes for a different reason, depends on a
different subsystem, or operates at a different trust boundary.

A large file is not automatically a monolith. A file has mixed responsibility
when, for example, it combines several of these concerns:

- domain types and validation;
- model- or user-visible projection;
- privileged host-only state;
- persistence and SQL mapping;
- orchestration or lifecycle policy;
- transport, IPC or presentation;
- unrelated feature tests and fixtures.

Split mixed responsibilities along those boundaries. Do not split cohesive code
only to meet a line-count target: excessive fragmentation makes invariants and
control flow harder to see.

## Crates and dependency direction

- A crate owns a stable subsystem boundary, not merely a collection of related
  files.
- Keep privileged effects behind the narrow crate that owns them. Value types
  and traits may cross boundaries; capabilities, secrets and concrete privileged
  implementations should not leak across them.
- Dependencies must follow the direction documented in the Rust workspace guide.
  A new reverse dependency requires an architecture decision, not a convenience
  import.
- Prefer extending an existing owner over creating a second source of truth.
- Composition belongs at the application boundary. Leaf crates should not know
  about the desktop UI or concrete sibling implementations.

## Modules and files

Start with the smallest structure that keeps a responsibility clear:

1. Use one file for a small, cohesive module.
2. Introduce private helper functions and types while they still share the same
   invariants and change axis.
3. Promote the module to a directory when it contains distinct responsibilities
   that deserve names of their own.
4. Keep the original module as a thin facade when callers benefit from a stable
   public API.

A feature-directory facade should normally:

- declare focused private submodules;
- re-export only the intended public surface;
- keep implementation details private or `pub(crate)`/`pub(super)` at the
  narrowest useful visibility;
- avoid forwarding functions that add no boundary, invariant or stable API.

Good split names describe responsibility, such as `projection`, `source`,
`record`, `validation`, `retention` or `migration`. Avoid catch-all destinations
such as `helpers`, `utils`, `common` or `misc` unless they represent a genuinely
stable shared abstraction.

Consider a split when a file:

- serves multiple audiences or trust levels;
- mixes domain policy with storage, transport or UI mechanics;
- has clusters of imports used by separate sections;
- requires distant sections to understand one operation;
- attracts unrelated fixtures and tests;
- creates frequent merge conflicts between independent work;
- cannot be named precisely in one short phrase.

When splitting, preserve behaviour and the public API first. Move one coherent
responsibility at a time, then run formatting, focused tests and workspace-wide
checks.

## Test placement

Tests follow the implementation's module boundary rather than one universal file
layout.

- For a small leaf module, keep focused unit tests inline in
  `#[cfg(test)] mod tests`.
- For a multi-file feature module, place its unit tests in `feature/tests.rs` and
  declare `#[cfg(test)] mod tests;` from the feature facade or owning module.
- For a substantial single-file module whose tests would obscure the
  implementation, use a clearly adjacent private test module; prefer promoting
  to a feature directory when that also clarifies implementation ownership.
- Put tests that exercise the public behaviour of several modules or crates in a
  crate-level `tests/` integration suite.
- Keep test fixtures with the narrowest suite that owns them. Promote a fixture
  to shared test support only after multiple suites genuinely need the same
  behaviour.

Do not add feature-specific tests to a large generic test block merely because it
already has setup code. Small local fixture duplication is preferable to coupling
otherwise independent suites through broad test helpers.

Test names should describe observable behaviour and the important condition. Test
public outcomes and invariants; avoid asserting incidental private structure.
Security boundaries should include rejection, tampering and exact-boundary cases,
not only successful paths.

## APIs, types and visibility

- Make invalid states difficult to construct. Validate at the boundary and keep
  validated fields private when callers must not mutate them independently.
- Use domain-specific types for identities, capabilities, statuses and bounded
  values instead of passing loosely related strings and booleans.
- Prefer narrow return types and typed errors over sentinel values or stringly
  typed control flow.
- Public API is a maintenance commitment. Default to private visibility and
  widen it only for a known caller.
- Keep host-only authority structurally absent from model-visible, frontend-safe
  and diagnostic types. Redaction after serialisation is not a substitute for a
  safe type boundary.
- Avoid duplicating policy across layers. A lower-level exact guard may reinforce
  a higher-level admission check, but each layer's responsibility must be clear.

## Functions and control flow

- A function should operate at one level of abstraction. Extract parsing,
  validation, persistence mapping or effect execution when it distracts from the
  caller's main decision.
- Prefer explicit data flow over hidden mutation and ambient state.
- Fail closed at trust boundaries and preserve actionable typed errors.
- Keep limits and boundary conditions explicit, checked and tested.
- Comments should explain invariants, security reasoning or non-obvious tradeoffs.
  Do not narrate code that is already clear from its names and structure.

## Agent and model-facing changes

Read [agent and tool design](agent-design.md) when changing model-facing tool
contracts, orchestration, context handling or recovery. That reference retains
the detailed product rules; this guide covers contributor coding conventions.

## Persistence and migrations

- Repository methods expose domain operations; SQL encoding, row validation and
  retention mechanics belong in focused storage modules.
- Stored records must be validated when written and when reconstructed. Do not
  assume local persistence is inherently trustworthy or internally consistent.
- Schema changes are versioned, forward-incompatible versions fail safely, and
  migration behaviour is covered by tests.
- Foreign keys, cascade behaviour and retention rules are part of the domain
  contract and should be visible in both implementation and verification.

## Naming and documentation

- Names should identify the owned concept or action, not its implementation
  history. Avoid `new`, `legacy`, `misc` and numbered variants as permanent
  architecture.
- Use Rust documentation comments for public contracts, errors and security
  constraints. Keep internal decision records in `docs/` when the
  rationale spans modules or will guide future work.
- Update the relevant architecture or security document when implementation
  changes a documented contract.
- Do not leave obsolete alternatives described as if they are current.

## Build toolchain

Frontend development, tests and builds use Node.js **22.23.4**, pinned in
`.nvmrc`, and pnpm **10.2.0**. Run `nvm install` and `nvm use` from the checkout
when using nvm; install pnpm with `npm install --global pnpm@10.2.0` under that
Node version. Package engine checks reject other Node versions. CI uses the same
`.nvmrc`. Node is a build dependency and is not bundled with the desktop app.

## Change and review discipline

Keep changes scoped to one coherent outcome. Refactors should not silently change
behaviour; behaviour changes should have explicit tests and documentation when they
alter a contract.

Before considering Rust work complete, normally run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked -- --test-threads=1
git diff --check
```

Use focused tests during iteration, then the workspace-wide gates before handoff.
If a gate cannot run, record exactly which check was skipped and why.

Reviewers should be able to answer:

- What responsibility does each changed module own?
- Is the public surface smaller than the implementation surface?
- Are trust boundaries expressed in types and validation?
- Are tests located with the behaviour they own?
- Could an independent concern change without editing this module?
- Do names and documentation describe the current design?
- Do formatting, linting and tests pass without warnings?

Exceptions are allowed when they make the code safer or materially clearer.
Document the reason close to the exception or in the relevant architecture
record so it does not become an accidental precedent.
