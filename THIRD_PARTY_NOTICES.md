# Third party notices

The root Apache-2.0 licence covers original Minutus code. It does not relicense
third-party dependencies or downloaded model weights.

| Component | Licence and retained notice |
| --- | --- |
| Ollama | MIT; `src-tauri/ollama-runtime/OLLAMA-LICENSE` |
| Accelerated Qwen adaptations | Apache-2.0; public dependency LICENSE and NOTICE, copied into generated runtime notices |
| llama.cpp and ggml | MIT; notices retained by the native source and runtime builds |
| whisper.cpp | MIT; `src-tauri/speech-runtime/WHISPER-CPP-LICENSE.txt` |
| OpenAI Whisper | MIT; `src-tauri/speech-runtime/OPENAI-WHISPER-LICENSE.txt` |
| UI fonts | SIL Open Font License 1.1; `public/fonts/OFL-*.txt` |
| Rust and JavaScript dependencies | Individual package licences, identified by the lockfiles and release SBOM |

Model profiles record artefact identities and licence links in
`crates/model-runtime/catalogue/`. Download and use each model under its own
licence and usage conditions. No model weights are distributed with the source.

CC0-1.0 is approved through exact-version exceptions for bip39 2.2.2,
bitcoin_hashes 0.14.101 and hex-conservative 0.2.2. Its lack of patent grants is
explicitly accepted for this dependency chain; see [licence policy](docs/license-policy.md).
CC0 remains outside the global allowlist, and upgrades require a new review.

The unmaintained paste dependency (RUSTSEC-2024-0436) has a temporary,
[documented exception](docs/dependency-maintenance.md) for the current compile-time
dependency path, expiring on 2027-10-13. Other advisory checks stay enabled.
