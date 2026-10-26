# Qwen agent alignment

Minutus's Rust agent loop follows Qwen's structured conversation pattern,
but remains an independent desktop agent. It does not embed the Qwen-Agent Python
SDK, reproduce its full controller, or claim benchmark parity. This comparison
describes the bundled implementation; it does not establish
identical behaviour across other backends or deployments.

| Area | Minutus behaviour and remaining differences |
| --- | --- |
| Native tool loop | Structured assistant calls, paired tool results and continuation through the provider. Minutus validates names, arguments, IDs and call/result history itself. |
| Streaming | Thinking, visible content and tool calls remain separate native channels. Fragmented input, cancellation, malformed calls and terminal failures have focused tests. UI echo filtering changes presentation, not replay. |
| Native message history | Selected Qwen reasoning and complete call/result batches survive across turns within context bounds. This follows Qwen-Agent's message-level pattern of appending assistant output and paired tool results for continuation. Minutus selects and compacts its own bounded history; reasoning is excluded from summaries, recall, trusted evidence and ordinary diagnostics. Retained message fields alone do not establish which thinking a backend renders into the next prompt. |
| Continuation | Previously admitted host-state capsules are replayed in place within one request, only for an exact compatible message prefix, binding and image selection. Current state supersedes old capsules. This is Minutus-specific state management. |
| Sampling | Non-thinking uses temperature 0.7, top-p 0.8 and presence penalty 1.5. Thinking uses 1.0, 0.95 and 0. Both use top-k 20, min-p 0 and repeat penalty 1. These are the pinned app settings, not a claim of matching every publisher profile. |
| Thinking control | Quick explicitly disables thinking; Balanced/Deep map to native medium/high effort. This aligns the transport with native thinking controls, while the product's mode selection remains its own policy. The model-card agent example enables thinking rather than defining these desktop modes. |
| Generation limits | Qwen output is capped at 8,192 tokens per generation, shared by thinking, visible text and tool calls. Execution profiles can allocate less; some actions preserve the full ceiling. The reasoning reserve is context accounting, not a separately enforced thinking-token allowance. Reference deployments may use different budgets. |
| Controller and authority | Semantic planning, deferred tool discovery, operation ledgers, exact context admission, recovery limits, filesystem permissions and native payment approvals remain Minutus policies. The runtime remains pinned Ollama plus the qualified local backend, rather than the reference example's OpenAI-compatible deployment. |

## Retained history versus rendered thinking

Qwen-Agent's function-call controller appends assistant outputs and tool results
to its working messages. Its OpenAI-compatible adapter can carry a separate
`reasoning_content` field. That message-level retention is the relevant alignment
with Minutus.

The Qwen3.5 model card separately recommends omitting thinking from older user
turns in the rendered prompt. Its published chat template omits that thinking
before the latest user query while retaining thinking within the current tool
continuation. Message storage, provider transport and the final model template
are different stages. Minutus passes selected reasoning to the native Ollama
backend, which renders the model prompt. This source comparison does not prove
identical rendered prompts across that backend and the reference deployment.

For the app limits, see [execution profiles](../crates/agent-core/src/execution_profile.rs),
[round budgets](../crates/app-core/src/chat_generation/support.rs),
[history selection](../crates/app-core/src/chat_generation/history.rs) and the
[product output ceiling](../crates/app-core/src/lib.rs). The
[sampling implementation](../crates/model-runtime/src/ollama/sampling.rs)
defines the settings above.

## Validation scope

The public tests cover structured history, channel separation, tool continuation,
cancellation and malformed input. Opt-in live inference tests require their pinned
models and runtime fixtures; ordinary test passes do not exercise those cases.
No reproducible comparison against an official Qwen-Agent deployment is published
here, so this document makes no benchmark or speedup claim.

## Official references

- [Qwen-Agent function-call controller](https://github.com/QwenLM/Qwen-Agent/blob/main/qwen_agent/agents/fncall_agent.py).
- [Qwen-Agent input truncation and raw API path](https://github.com/QwenLM/Qwen-Agent/blob/main/qwen_agent/llm/base.py).
- [Qwen3.5 chat template](https://huggingface.co/Qwen/Qwen3.5-27B/blob/main/chat_template.jinja), including its latest-user boundary for rendered thinking.
- [Qwen-Agent OpenAI-compatible native channel adapter](https://github.com/QwenLM/Qwen-Agent/blob/main/qwen_agent/llm/oai.py).
- [Qwen3.5 27B model card](https://huggingface.co/Qwen/Qwen3.5-27B), including its sampling, historical-thinking guidance and agent example. The catalogue's Qwen3.8 runtime tag does not make every Qwen3.5 recommendation an identical deployment contract.
