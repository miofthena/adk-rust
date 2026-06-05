# Implementation Plan: Managed Agent Runtime

## Overview

This plan delivers the `adk-managed` crate — a new workspace member implementing the `ManagedAgentRuntime` trait and `DefaultManagedAgentRuntime`. Organized in 6 phases: types, trait, session loop, checkpoint/resume, provider parity, and golden fixture conformance.

## Tasks

- [ ] 1. Foundation: Crate scaffold and wire types
  - [ ] 1.1 Create `adk-managed` crate with Cargo.toml and module structure
    - Create `adk-managed/Cargo.toml` as a new workspace member with dependencies on adk-core, adk-agent, adk-model, adk-runner, adk-session, adk-tool, adk-skill, adk-telemetry, adk-sandbox (optional), adk-memory (optional)
    - Create `adk-managed/src/lib.rs` with module declarations
    - Create `adk-managed/src/types/mod.rs` with submodule re-exports
    - Add `"adk-managed"` to root `Cargo.toml` workspace members
    - Add `adk-managed = { workspace = true }` to `[workspace.dependencies]`
    - Verify `cargo check -p adk-managed` passes (empty crate)
    - _Requirements: 10.1_

  - [ ] 1.2 Implement `ContentBlock` type
    - Create `adk-managed/src/types/content.rs`
    - Implement `ContentBlock` enum: `Text { text }`, `Image { source }`, `File { file_id }`
    - Derive `Debug, Clone, Serialize, Deserialize`; use `#[serde(tag = "type")]`
    - Mark `#[non_exhaustive]`
    - Write unit tests for serialization round-trip matching CANON §3.5 wire shapes
    - _Requirements: 3.3, 10.2_

  - [ ] 1.3 Implement `ModelRef`, `Provider`, `ModelConfig` types
    - Create `adk-managed/src/types/model_ref.rs`
    - Implement `ModelRef` enum: `Shorthand(String)` | `Structured { provider, model, speed? }`
    - Implement `Provider` enum: `Gemini`, `Openai`, `Anthropic`, `Ollama`, `OpenaiCompatible`
    - Implement `ModelConfig` enum: `Name(String)` | `Compatible { model, base_url, api_key }`
    - Use `#[serde(untagged)]` for ModelRef, `#[serde(rename_all = "snake_case")]` for Provider
    - Write unit tests: shorthand parses, structured parses, openai_compatible with base_url
    - _Requirements: 3.4, 9.1_

  - [ ] 1.4 Implement `ToolConfig`, `McpServerConfig`, `SkillRef`, `PermissionPolicy` types
    - Create `adk-managed/src/types/tools.rs`
    - Implement all tool-related types matching CANON §3.7, §3.8, §3.9
    - `ToolConfig`: built-in enum variants + `Custom { name, description?, input_schema }`
    - `McpServerConfig`: `{ name, transport, command?, args?, url?, env?, auto_approve? }`
    - `SkillRef`: `{ skill_id }`
    - `PermissionPolicy`: `{ default: PermissionMode, tools?: HashMap<String, PermissionMode> }`
    - `PermissionMode`: `AutoApprove`, `Prompt`, `Deny`
    - Write unit tests for serialization matching CANON wire shapes
    - _Requirements: 3.1, 3.3_

  - [ ] 1.5 Implement `ManagedAgentDef` type
    - Create `adk-managed/src/types/agent_def.rs`
    - Implement `ManagedAgentDef` struct with all fields from design doc
    - Mark `#[non_exhaustive]`
    - Write unit test: serialize a full def, verify JSON matches CANON §3.1
    - _Requirements: 3.1, 3.3, 10.2_

  - [ ] 1.6 Implement `UserEvent` enum
    - Create `adk-managed/src/types/events.rs`
    - Implement all 6 variants with exact CANON §3.4 `type` strings
    - `user.message`, `user.interrupt`, `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result`, `user.define_outcome`
    - Mark `#[non_exhaustive]`
    - Write unit tests: each variant serializes to correct JSON; unknown type rejected
    - _Requirements: 4.2, 10.2_

  - [ ] 1.7 Implement `SessionEvent` enum
    - In `adk-managed/src/types/events.rs`
    - Implement all 7 variants with exact CANON §3.4 `type` strings + `seq: u64`
    - `agent.message`, `agent.tool_use`, `agent.custom_tool_use`, `agent.mcp_tool_use`, `status.running`, `status.idle`, `error`
    - Add `stop_reason: Option<StopReason>` to the `StatusIdle` variant
    - Define `StopReason` enum: `EndTurn`, `RequiresAction { event_ids: Vec<String> }`, `MaxTokens`
    - Mark both `SessionEvent` and `StopReason` as `#[non_exhaustive]`
    - Write unit tests: each variant serializes with correct type and seq; seq is strictly increasing; stop_reason serializes correctly
    - _Requirements: 4.1, 4.7, 10.2_

  - [ ] 1.8 Implement `SessionStatus` enum and `RuntimeError`
    - Create `adk-managed/src/types/session.rs` and `adk-managed/src/types/error.rs`
    - `SessionStatus`: `Queued`, `Running`, `Idle`, `Rescheduling`, `Paused`, `Completed`, `Failed`, `Archived`
    - `Rescheduling` represents a transient error with auto-retry; transitions: Running → Rescheduling → Running (success) or Failed (exhaust)
    - `RuntimeError`: all variants from design doc (InvalidRequest, NotFound, Conflict, ProviderError, ToolTimeout, CheckpointFailed, SandboxError, Internal)
    - Write unit tests for status serialization (including Rescheduling) and error Display
    - _Requirements: 2.6, 2.7, 10.2_

- [ ] 2. Checkpoint — Verify types compile and serialize correctly
  - Run `cargo check -p adk-managed`
  - Run `cargo test -p adk-managed`
  - Verify: all types serialize/deserialize round-trip to CANON wire shapes

- [ ] 3. Trait and ModelRef Resolver
  - [ ] 3.1 Define `ManagedAgentRuntime` trait
    - Create `adk-managed/src/runtime.rs`
    - Define the async trait with all methods from design doc
    - Define `AgentHandle` and `SessionHandle` as opaque ID wrappers
    - Verify `cargo check -p adk-managed` passes
    - _Requirements: 1.1, 1.2_

  - [ ] 3.2 Define `ModelResolver` trait and implement `DefaultModelResolver`
    - Create `adk-managed/src/resolver.rs`
    - Define `ModelResolver` async trait with `resolve(&self, model_ref: &ModelRef) -> Result<Arc<dyn Llm>>`
    - Implement `DefaultModelResolver` that:
      - Shorthand → infer provider from name (gemini-* → Gemini, gpt-* → OpenAI, claude-* → Anthropic)
      - Structured → use provider field directly
      - OpenaiCompatible → construct OpenAI-compatible client with base_url + api_key
    - Write unit tests with mock model construction
    - _Requirements: 9.1, 9.2, 9.3_

  - [ ] 3.3 Implement `ManagedAgentDef` → runnable agent builder
    - Create `adk-managed/src/agent_builder.rs`
    - Implement `build_agent(def: &ManagedAgentDef, model: Arc<dyn Llm>) -> Result<Arc<dyn Agent>>`
    - Wire: model + system prompt → LlmAgentBuilder, tools → tool registration, MCP → McpToolset, skills → skill injection, permission_policy → ToolConfirmationPolicy
    - _Requirements: 3.2_

- [ ] 4. Checkpoint — Verify trait and resolver compile
  - Run `cargo check -p adk-managed`
  - Run `cargo test -p adk-managed`

- [ ] 5. Session Loop and Event System
  - [ ] 5.1 Implement monotonic sequence counter
    - Create `adk-managed/src/sequence.rs`
    - `SequenceCounter` with `next() -> u64` (atomic, starts at 0)
    - Thread-safe (AtomicU64)
    - _Requirements: 4.1_

  - [ ] 5.2 Implement custom tool parking lot
    - Create `adk-managed/src/parking.rs`
    - `ToolParkingLot` with `park(tool_use_id) -> Result<Vec<ContentBlock>>` and `deliver(tool_use_id, content) -> Result<()>`
    - Configurable timeout (default 5 minutes)
    - On timeout: return RuntimeError::ToolTimeout
    - Uses oneshot channels internally
    - _Requirements: 4.3_

  - [ ] 5.3 Implement checkpoint manager
    - Create `adk-managed/src/checkpoint.rs`
    - `CheckpointManager` wrapping SessionService
    - `checkpoint(session_id, event, run_state)` — atomic persist
    - `load_checkpoint(session_id)` — load last state for resume
    - `RunState`: current seq, pending parking ids, session status
    - _Requirements: 2.2, 2.5_

  - [ ] 5.4 Implement the supervised session loop
    - Create `adk-managed/src/session_loop.rs`
    - `SessionLoop` struct that:
      - Runs in a `tokio::spawn`ed task
      - Dequeues UserEvents from an mpsc channel
      - Invokes Runner per turn
      - Maps Runner events to SessionEvent variants
      - Assigns seq from SequenceCounter
      - Checkpoints atomically after each event
      - Broadcasts to stream subscribers (tokio::broadcast)
      - Parks on custom_tool_use (via ToolParkingLot)
      - Handles interrupt (CancellationToken)
      - Handles pause/resume (Notify + status flag)
    - Emit `status.running` at turn start, `status.idle` at turn end
    - On interrupt mid-turn: stop at next boundary, emit `status.idle`
    - _Requirements: 2.1, 2.4, 4.1, 4.3, 4.5_

  - [ ] 5.5 Implement event replay (stream_events with from_seq)
    - In the session loop / broadcast system:
      - Maintain an ordered event log (Vec<SessionEvent> or persisted)
      - `stream_events(from_seq=Some(k))`: replay events with seq>k from log, then attach to live broadcast
      - `stream_events(from_seq=None)`: live tail only
    - _Requirements: 4.4_

- [ ] 6. Checkpoint — Verify session loop compiles
  - Run `cargo check -p adk-managed`
  - Run unit tests for sequence, parking, checkpoint

- [ ] 7. DefaultManagedAgentRuntime Implementation
  - [ ] 7.1 Implement `DefaultManagedAgentRuntime` struct and constructor
    - Create `adk-managed/src/default_runtime.rs`
    - Fields: model_resolver, session_service, sandbox_factory (optional), memory (optional), sessions map
    - Constructor: `new(resolver, sessions, sandbox?, memory?)`
    - _Requirements: 1.3, 1.4_

  - [ ] 7.2 Implement `create()` method
    - Resolve ModelRef → Arc<dyn Llm>
    - Build agent from ManagedAgentDef
    - Store agent in internal registry
    - Return AgentHandle
    - _Requirements: 1.1_

  - [ ] 7.3 Implement `start_session()` method
    - Create session via SessionService
    - Initialize SequenceCounter, ToolParkingLot, CheckpointManager
    - Spawn SessionLoop in background task
    - Set initial status to `Queued`
    - Store ActiveSession handle
    - _Requirements: 1.1, 2.1, 2.6_

  - [ ] 7.4 Implement `send_event()` method
    - Validate UserEvent type is known (reject unknown with error)
    - If `user.tool_result` in hosted mode → reject (self-hosted only caveat)
    - If `user.custom_tool_result` → deliver to parking lot
    - If `user.tool_confirmation` → deliver to confirmation handler
    - If `user.message` → enqueue to session loop
    - If `user.interrupt` → signal interrupt
    - _Requirements: 4.2, 4.6_

  - [ ] 7.5 Implement `stream_events()` method
    - Subscribe to session's broadcast channel
    - If `from_seq` provided, replay historical events first
    - Return merged stream (replay + live)
    - _Requirements: 4.4_

  - [ ] 7.6 Implement `interrupt()`, `pause()`, `resume()`, `status()`, `archive()`, `delete_session()`
    - Interrupt: signal CancellationToken on the session loop
    - Pause: set pause flag + checkpoint
    - Resume: clear pause flag + notify loop
    - Status: read from ActiveSession state
    - Archive: set terminal status, stop loop
    - Delete: archive + remove data
    - _Requirements: 1.1, 2.4_

- [ ] 8. Checkpoint — Verify DefaultManagedAgentRuntime compiles
  - Run `cargo check -p adk-managed`
  - Run `cargo test -p adk-managed`

- [ ] 9. Provider Parity and Schema Normalization
  - [ ] 9.1 Implement provider-neutral event mapping from Runner output
    - In session_loop.rs, map Runner's `Event` stream to `SessionEvent` variants uniformly:
      - LLM text response → `agent.message`
      - Function call → `agent.tool_use` (built-in) or `agent.custom_tool_use` (custom) or `agent.mcp_tool_use` (MCP)
      - Tool confirmation request → parking
    - This mapping MUST produce identical type sequences regardless of provider
    - _Requirements: 5.1, 5.5_

  - [ ] 9.2 Verify schema normalization across providers
    - For each provider (gemini, openai, anthropic, ollama, openai_compatible):
      - Ensure MCP tool schemas with `$schema`/`additionalProperties` are normalized
      - Use existing `SchemaAdapter` per provider
      - Write test: same MCP tool schema → callable by each provider
    - _Requirements: 5.2_

  - [ ] 9.3 Implement uniform usage reporting
    - After each turn, extract `input_tokens` and `output_tokens` from `LlmResponse.usage`
    - Emit as part of session metadata (or a usage event)
    - Ensure all providers report usage uniformly
    - _Requirements: 5.3_

- [ ] 10. Checkpoint — Verify provider parity
  - Run fixture F-8 (provider parity matrix) — MUST pass
  - Verify identical event-type sequences across all 5 providers with mock Llm doubles

- [ ] 11. Golden Fixture Conformance Tests
  - [ ] 11.1 Create fixture JSON files (F-1 through F-8)
    - Create `adk-managed/tests/fixtures/` directory
    - Write F-1 (hello, no tools), F-2 (MCP tool), F-3 (custom tool round-trip), F-4 (confirmation deny), F-5 (resume after kill), F-6 (replay from seq), F-7 (interrupt mid-turn), F-8 (provider parity matrix)
    - Each fixture: `{ name, agent_def, mock_responses, user_events, expect_sequence }`
    - _Requirements: 10.5_

  - [ ] 11.2 Implement fixture test runner
    - Create `adk-managed/tests/fixture_conformance_tests.rs`
    - Load each fixture JSON, construct DefaultManagedAgentRuntime with mock Llm
    - Execute the flow: create → start_session → send_events → collect stream
    - Assert `event.type` sequence matches `expect_sequence`
    - _Requirements: 10.5_

  - [ ] 11.3 Implement F-5 (resume after kill) test
    - Special handling: run to parking state, drop the runtime, reconstruct, call resume
    - Assert continuation from checkpoint
    - _Requirements: 2.3, 10.5_

  - [ ]* 11.4 Write property tests (P-1 through P-7)
    - Create `adk-managed/tests/property_tests.rs`
    - P-1: Seq monotonicity — for random event sequences, seq always increases
    - P-2: Checkpoint atomicity — interleaved crash/resume produces consistent logs
    - P-3: Resume no-gap — after resume, no seq≤N re-emitted
    - P-4: Parking timeout — custom_tool_use always resolves (result or timeout)
    - P-5: Provider parity — same def + same mock → identical type sequences across providers
    - P-6: Replay completeness — from_seq=k returns exactly seq>k events
    - P-7: State machine validity — only valid transitions succeed
    - _Requirements: 10.4_

- [ ] 12. Checkpoint — All fixtures pass
  - Run `cargo test -p adk-managed`
  - All F-1 through F-8 pass
  - All property tests pass
  - Run `cargo clippy -p adk-managed -- -D warnings`
  - Run `cargo fmt -p adk-managed -- --check`

- [ ] 13. Integration, Documentation, and Wiring
  - [ ] 13.1 Wire adk-managed into the umbrella crate
    - Add `managed-runtime = ["dep:adk-managed"]` feature to `adk-rust/Cargo.toml`
    - Re-export key types behind the feature flag
    - Verify `cargo check -p adk-rust --features managed-runtime`
    - _Requirements: 1.5, 10.1_

  - [ ] 13.2 Create example binary (smoke test for platform team)
    - Create `examples/managed_runtime_hello/` standalone crate
    - Runs fixture F-1 end-to-end: create agent, start session, send message, collect events
    - Uses mock Llm (no API key required)
    - Platform team can clone and smoke-test integration
    - _Requirements: 10.5 (definition of done §14.5)_

  - [ ] 13.3 Update documentation
    - Update CHANGELOG.md with the new feature
    - Update AGENTS.md with adk-managed description
    - Create `docs/official_docs/managed-agents/runtime.md`
    - Add STABILITY note: additive, feature-gated, experimental
    - _Requirements: 10.1_

  - [ ] 13.4 Backward compatibility verification
    - `cargo check -p adk-runner` (unchanged)
    - `cargo check -p adk-agent` (unchanged)
    - `cargo check -p adk-session` (unchanged)
    - `cargo test -p adk-runner`
    - `cargo test -p adk-agent`
    - Verify no `ep-*` dependency in the dependency tree
    - _Requirements: 10.1, 10.3_

- [ ] 14. Final checkpoint — Full conformance
  - All golden fixtures F-1 through F-8 pass
  - All property tests P-1 through P-7 pass
  - `cargo clippy -p adk-managed -- -D warnings` clean
  - `cargo fmt --all -- --check` clean
  - `cargo check -p adk-rust --features managed-runtime`
  - Backward compat: `cargo test -p adk-runner -p adk-agent -p adk-session`
  - Example runs: `cargo run --manifest-path examples/managed_runtime_hello/Cargo.toml`

## Notes

- Tasks marked with `*` are optional property tests that can be deferred
- The `adk-managed` crate is a NEW workspace member, not a module inside an existing crate
- No `ep-*` platform crate dependency is permitted (R-9.3 hard constraint)
- The runtime receives resolved API keys (plaintext), never credential refs — the platform resolves those
- `user.tool_result` is self-hosted only; hosted topology executes built-in tools in the sandbox
- Initial session status is `Queued` (not `Idle`) per CANON §3.3 state machine
- `Last-Event-ID` is an HTTP header for SSE reconnection (platform's concern); the runtime exposes `stream_events(from_seq)` which the platform maps to the header
- F-8 (provider parity) is the PRIMARY correctness gate — prioritize it early as a design forcing-function
- All types are `#[non_exhaustive]` where polymorphic (R-9.2)

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "1.3", "1.4", "1.5", "1.6", "1.7", "1.8"] },
    { "id": 2, "tasks": ["3.1", "3.2", "3.3"] },
    { "id": 3, "tasks": ["5.1", "5.2", "5.3"] },
    { "id": 4, "tasks": ["5.4", "5.5"] },
    { "id": 5, "tasks": ["7.1"] },
    { "id": 6, "tasks": ["7.2", "7.3", "7.4", "7.5", "7.6"] },
    { "id": 7, "tasks": ["9.1", "9.2", "9.3"] },
    { "id": 8, "tasks": ["11.1", "11.2", "11.3", "11.4"] },
    { "id": 9, "tasks": ["13.1", "13.2", "13.3", "13.4"] }
  ]
}
```
