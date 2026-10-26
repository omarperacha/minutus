# Bundled application helpers

`scripts/build-llama-sidecar.sh` creates the target-triple-suffixed executable
that Tauri packages from this directory:

`zeta-tokenizer-aarch64-apple-darwin`

The executable is a generated release artefact and is intentionally ignored by
Git. It is a vocabulary-only tokeniser built statically from the exact
`llama.cpp` commit recorded in the script. It cannot generate text, bind a
network listener, or load model weights.

`scripts/build-computation-runner.sh` creates:

`zeta-computation-runner-aarch64-apple-darwin`

`zeta-computation-broker-aarch64-apple-darwin`

The runner embeds the Cargo-locked QuickJS-NG runtime. In release packaging it
lives inside the independently sandboxed computation XPC service and inherits
that service's deny-by-default sandbox. The small broker inherits the main app
sandbox but contains no interpreter; it only forwards one authenticated bounded
frame to the private XPC service.

The build writes exact runner and broker digest manifests and stages the XPC
bundle with its own complete payload manifest. Release compilation verifies all
three artefacts before Tauri consumes them.
`../computation-runtime.lock` pins the reviewed interpreter, Cargo checksums,
target, limits, signed identifier and entitlement-policy digest. Bundle signing
writes a digest of the signed nested helper into the outer signed application;
the release verifier checks both that digest and the nested signature.

`scripts/asr/stage_helper.py` creates the target-suffixed
`zeta-whisper-helper` used for local dictation. It builds from the digest-pinned
whisper.cpp archive, records the staged executable digest and rejects unexpected
host/target or dynamic-library combinations. `../speech-runtime/runtime.lock`
pins the build inputs, protocol, selected model identity, macOS deployment target,
permission metadata, helper sandbox and bundled licence notices.
