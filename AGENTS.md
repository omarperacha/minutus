# Minutus repository instructions

These instructions apply to the entire repository. Read
`docs/engineering-house-style.md` and `crates/README.md` before making
architectural or substantial Rust changes. The workspace boundaries documented
in `crates/README.md` are authoritative. For model-facing tools, orchestration,
context handling or recovery changes, also read `docs/agent-design.md`.

## Ownership and structure

- Organise code around one coherent responsibility and reason to change.
- Keep privileged state and effects inside the crate and module that own their
  trust boundary. Do not reverse a documented crate dependency for convenience.
- Treat crate roots and feature `mod.rs` files as thin facades: module
  declarations, intentional re-exports, stable boundary types, and essential
  composition or startup wiring. Put feature behaviour in focused owning modules.
- Promote a feature to a directory when it has independently named
  responsibilities. Do not create `helpers`, `utils`, `common`, or `misc`
  catch-alls.
- Prefer private visibility. Widen visibility only for a known caller, and keep
  host authority absent from model-visible, IPC-safe, and diagnostic types.
- Preserve behaviour and the public API during structural refactors unless the
  task explicitly changes a contract.

Before adding code to an existing file, identify the responsibility being
changed. If that responsibility already has an owning module, extend it there.
Do not add behaviour to a crate root merely because it can access every
dependency.

## Tests and documentation

- Keep focused unit tests beside the module that owns the behaviour. Use
  `feature/tests.rs` for a multi-file feature and crate-level integration tests
  only for behaviour spanning module boundaries.
- Do not add feature-specific tests to a large generic test block for access to
  existing fixtures; duplicate small local setup when that preserves ownership.
- Update the relevant architecture or security document when implementation changes an
  architectural, security, or model-facing contract. Remove obsolete
  descriptions rather than leaving competing current designs.
- Explain any house-style exception beside the exception or in the relevant
  architecture record, including why it is safer or materially clearer.

## Completion checks

For Rust changes, run focused tests while iterating and, before handoff, normally
run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
git diff --check
```

Also run `pnpm check` when frontend or generated IPC bindings may be affected.
Report any skipped or failing check exactly; do not weaken a workspace lint or
test to make an unrelated change pass.
