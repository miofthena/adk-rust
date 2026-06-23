# CodeAgent (CodeAct)

`CodeAgent` is a peer to [`LlmAgent`](llm-agent.md) that **acts by writing and
running code** instead of emitting one tool call at a time. Each turn the model
produces a single script; tools are exposed as callable functions the script can
compose; and the script communicates its result by returning a tagged value.

This is the *CodeAct* pattern: rather than `call tool A → observe → call tool B`,
the model writes `b(a(x))` in one script, so multi-step work happens in a single
turn. It is enabled by the `codeact` feature on `adk-agent`.

## When to use it

- Tasks that chain or combine several tools per turn (data wrangling, batch
  operations, glue logic).
- Models post-trained for code generation.
- Workflows where a real interpreter (e.g. Python) is available as the action
  substrate.

For native tool-calling, prefer [`LlmAgent`](llm-agent.md). For a sandboxed
file/shell coding harness, see the [Coding Agent](../coding-agent/index.md).

## How the loop works

Each turn:

1. The model emits one fenced code block (a script).
2. The script runs on a [`CodeRuntime`]; tool calls surface to the host, which
   executes the tool and resumes the script with the result.
3. The script returns a tagged `ScriptOutput`:
   - `observation` — fed back to the model; the loop continues.
   - `error` — fed back as a message; the loop continues.
   - `final_result` — returned to the caller; the loop ends.
   - `transfer_to_agent` — hands control to another agent; the loop ends.

The framework is **language-agnostic**: the `CodeRuntime` trait is the step-wise
interpreter seam, and it reports its own language/environment to the model via a
freeform prompt. The intended production adapter wraps
[Monty](https://github.com/pydantic/monty), a Rust-native Python interpreter.

## Durability: suspend and resume

`CodeAgent` is stateless across invocations — durable state lives in the
**session**, exactly like `LlmAgent`. Two situations *suspend* the run:

- a confirmation-gated tool with no decision yet (HITL), and
- a long-running tool whose result arrives out-of-band.

On suspension, the live interpreter continuation is serialized into a
`CodeActCheckpoint` and written to session state; the next `run()` reads it back
and resumes — the confirmation decision arrives via
`RunConfig::tool_confirmation_decisions`, and a long-running result arrives as a
`FunctionResponse` in the next message. Inline tool calls are bracketed with
write-ahead (SAVE-BEFORE) and SAVE-AFTER checkpoints: once the SAVE-AFTER
checkpoint is persisted, recovery resumes with the stored result and never
re-runs the tool. A crash in the narrow window after a tool's side effect but
before its SAVE-AFTER checkpoint lands will re-run the tool on recovery, so
tools that are not idempotent should guard against that (the same at-least-once
boundary as `LlmAgent`).

This requires a runtime that can snapshot a paused call. A runtime that cannot
runs long-running tools inline and rejects confirmation pauses.

## Building a CodeAgent

```rust,ignore
use adk_agent::codeact::CodeAgent;
use std::sync::Arc;

// `model` implements `adk_core::Llm`; `runtime` implements `CodeRuntime`.
let agent = CodeAgent::builder()
    .name("analyst")
    .model(model)
    .runtime(runtime)
    .instruction("Prefer concise, composable steps.")
    .tool(Arc::new(load_csv_tool))
    .output_key("report")
    .build()?;
```

`model` and `runtime` are required; everything else has a default.

## Parity with LlmAgent

The builder mirrors `LlmAgentBuilder`:

- **Model**: `generate_content_config` plus `temperature`/`top_p`/`top_k`/
  `max_output_tokens` shorthands.
- **Instructions**: `instruction`/`instruction_provider`,
  `global_instruction`/`global_instruction_provider`, with `{state.key}`
  template injection; plus skills (`skills` feature).
- **History**: `include_contents`.
- **Tools**: static `tool`s and per-invocation `toolset`s; `tool_timeout`,
  `default_retry_budget`/`tool_retry_budget`, `circuit_breaker_threshold`, and
  `on_tool_error` fallbacks.
- **Authorization**: `ToolConfirmationPolicy`
  (`require_tool_confirmation`/`require_tool_confirmation_for_all`).
- **Transfer**: `sub_agent`s and `disallow_transfer_to_parent`/
  `disallow_transfer_to_peers`.
- **Output**: `output_key`, `output_schema`/`output_type` with a
  correction-retry loop (`output_max_retries`).
- **Callbacks**: `before_callback`/`after_callback`,
  `before_model_callback`/`after_model_callback`, and
  `before_tool_callback`/`after_tool_callback`/`after_tool_callback_full`.
  After-tool callbacks can inspect structured execution metadata via
  `CallbackContext::tool_outcome()`.
- **Feature-gated**: input/output guardrails (`guardrails`) and the
  `EnhancedPlugin` pipeline (`enhanced-plugins`).

Each tool call gets a fresh `ToolContext` that carries the interpreter call id
and delegates artifacts, memory, shared state, user scopes, and secrets to the
live invocation — so a tool behaves identically under `CodeAgent` or `LlmAgent`.

### Deliberate differences

- Code-execution sandboxing is the `CodeRuntime`'s responsibility, not a
  bolt-on.
- Tool dispatch is **sequential by design** (a single continuation is
  snapshotted at one call boundary), so there is no parallel
  `tool_execution_strategy`.
- There is no `skip_summarization` builder option — the model ends the loop
  itself via `final_result` — though a tool that sets `skip_summarization` on its
  actions still ends the run.

## Example

A runnable, dependency-free end-to-end demo — a self-contained `CodeRuntime` plus
a deterministic model — lives in
[`examples/codeact_agent`](https://github.com/zavora-ai/adk-rust/tree/main/examples/codeact_agent):

```bash
cargo run --manifest-path examples/codeact_agent/Cargo.toml
```

## Implementing a CodeRuntime

A `CodeRuntime` parses and steps a script, surfacing one external call at a time:

```rust,ignore
pub trait CodeRuntime: Send + Sync {
    fn start(&self, script: &str, script_name: &str) -> Result<RunStep, RuntimeError>;
    fn resume(&self, snapshot: &[u8], with: ResumeWith) -> Result<RunStep, RuntimeError>;
    fn capabilities(&self) -> RuntimeCapabilities { /* default */ }
    fn render_tools(&self, tools: &[Arc<dyn Tool>]) -> String { /* default */ }
}
```

- `RunStep::Call` surfaces exactly one pending call; resume it with a value or an
  error, or `dump()` its continuation to suspend.
- `RuntimeCapabilities::supports_suspension` must be `true` to enable HITL and
  long-running deferral; `prompt` describes the language/environment to the model.

See `examples/codeact_agent/src/runtime.rs` for a complete, minimal
implementation that supports suspend/resume.
