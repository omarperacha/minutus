# Bundled inference runtime

Run `python3 scripts/bootstrap.py` from the repository root to build the pinned
public accelerated Qwen source and stage the complete verified Ollama runtime.
No model weights are downloaded. See [build instructions](../../docs/build.md).

Generated native files and payload manifests are ignored. The reviewed official
artefact pin is retained separately from local rebuild provenance. Stock fallback
and hardware admission remain host policy. Do not swap backends in a live session.
