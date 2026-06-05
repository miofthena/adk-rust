# Requirements Document

## Introduction

This feature delivers the `ManagedAgentRuntime` — a provider-neutral, durable, resumable agent execution engine that enables ADK-Rust to **be** the managed agent service (not merely a client of someone else's). It unifies existing shipping components (`Runner`, `SessionService`, `SandboxRunner`, MCP, Skills, Memory, Telemetry) behind a single lifecycle trait that the enterprise platform hosts.

The runtime is the engine inside the managed service. It takes a declarative agent definition, builds a live agent, runs it durably (survives crashes), and emits a uniform event stream regardless of which LLM provider powers it. The platform wraps it with HTTP routes, auth, billing, and multi-tenancy.

**Normative source:** `00-CANONICAL-API.md` (CANON). Wire shapes, event names, and error types defined there are authoritative. If this document disagrees with CANON, CANON wins.

**Division of labor (hard constraint):** adk-rust owns provider-neutral single-agent runtime mechanics. The platform owns fleet governance (multi-tenancy, vault, billing, RBAC). adk-rust MUST NOT depend on any platform crate (`ep-*`). Dependency direction is platform → adk-rust only.

## Glossary

- **ManagedAgentRuntime**: The central async trait defining the lifecycle interface: create agents, start sessions, send/receive events, pause/resume/interrupt, archive.
- **DefaultManagedAgentRuntime**: The default implementation composed from `Runner` + pluggable `SessionService` + optional `SandboxRunner`, with no platform dependencies.
- **ManagedAgentDef**: The declarative agent definition (model + system prompt + tools + MCP servers + skills + permissions). Created once, referenced by many sessions.
- **ModelRef**: Provider-neutral model reference. Either a shorthand string (`"gemini-2.5-flash"`) or a structured object (`{provider, model, speed?}`). Supports gemini, openai, anthropic, ollama, and openai_compatible (with base_url + api_key).
- **UserEvent**: Client→agent events discriminated by `type`: `user.message`, `user.interrupt`, `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result` (self-hosted only), `user.define_outcome`.
- **SessionEvent**: Agent→client events discriminated by `type`: `agent.message`, `agent.tool_use`, `agent.custom_tool_use`, `agent.mcp_tool_use`, `status.running`, `status.idle`, `error`. Each carries a monotonic `seq`.
- **SessionStatus**: Lifecycle state enum: `Queued`, `Running`, `Idle`, `Paused`, `Completed`, `Failed`, `Archived`.
- **ContentBlock**: Union type for message content: `text`, `image`, `file`. Unknown types are opaque (forward-compatible).
- **ToolConfig**: Tool declaration: built-in (`bash`, `filesystem`, `web_search`, `web_fetch`, `code_execution`) or custom (client-executed, with `name`, `description`, `input_schema`).
- **McpServerConfig**: MCP server declaration: `{name, transport, command?, args?, url?, env?, auto_approve?}`.
- **SkillRef**: Reference to a skill package: `{skill_id}`.
- **PermissionPolicy**: Tool permission configuration: `{default: auto_approve|prompt|deny, tools?: {name: mode}}`.
- **Custom_Tool_Parking**: When the agent emits `agent.custom_tool_use`, the loop parks (waits) until the matching `user.custom_tool_result` arrives or a timeout elapses.
- **Event_Replay**: The ability to stream events starting from a given sequence ID, enabling SSE `Last-Event-ID` reconnection.
- **Provider_Parity**: The guarantee that an identical `ManagedAgentDef` produces an identical `SessionEvent` type sequence across all supported providers (fixture F-8).

## Requirements

### Requirement 1: ManagedAgentRuntime Trait

**User Story:** As the platform team, I want a single async trait that encapsulates the full managed agent lifecycle, so that I can host any agent regardless of provider behind a uniform API.

#### Acceptance Criteria

1. THE `adk-managed` module SHALL define an async trait `ManagedAgentRuntime` with methods: `create(def) -> AgentId`, `start_session(agent_id, env?) -> SessionId`, `send_event(session_id, UserEvent)`, `stream_events(session_id, from_seq?) -> EventStream`, `interrupt(session_id)`, `pause(session_id)`, `resume(session_id)`, `status(session_id) -> SessionStatus`, `archive(session_id)`, `delete_session(session_id)`.
2. THE trait SHALL be provider-agnostic: it takes a `ModelRef` (CANON §3.6) and SHALL behave identically for gemini, openai, anthropic, ollama, and openai_compatible.
3. THE module SHALL provide a `DefaultManagedAgentRuntime` composed from `Runner` + a pluggable `SessionService` + an optional `SandboxRunner` + an optional `Memory`, with no platform dependencies.
4. THE runtime SHALL accept injected services so the platform can supply its own `SessionService`, sandbox client, memory, and telemetry.
5. THE module SHALL be gated behind a feature flag (`managed-runtime`) so non-managed consumers are unaffected. WHEN the feature is off, no new public surface is exposed and `Runner`/`LlmAgent` are unchanged.

### Requirement 2: Durable Resumable Sessions

**User Story:** As a developer, I want my agent sessions to survive process crashes and resume from the last checkpoint, so that long-running tasks don't lose progress.

#### Acceptance Criteria

1. THE runtime SHALL run a session as a supervised background task, not tied to a single HTTP request lifecycle.
2. THE runtime SHALL persist run-state (current step, pending tool-call/confirmation waits, session status) sufficient to resume from any consistent checkpoint.
3. WHEN `resume(session_id)` is called after a process restart, THE runtime SHALL rehydrate run-state from the `SessionService` and continue from the last consistent checkpoint with no loss of committed events.
4. WHEN `pause(session_id)` is called, THE runtime SHALL stop consuming new input and checkpoint the current state. WHEN `resume` is subsequently called, THE runtime SHALL return to active processing.
5. Checkpoint writes SHALL be atomic with event emission so a crash cannot leave an event emitted but un-checkpointed (or vice versa).
6. WHEN a session is first created, THE initial status SHALL be `Queued` (not `Idle`). The state machine is: `Queued` → `Running` → `Idle` (per turn) → `Paused`/`Completed`/`Failed`/`Archived`.
7. THE `SessionStatus` enum SHALL include a `Rescheduling` variant for transient errors that will auto-retry. The state machine transitions: `Running` → `Rescheduling` → `Running` (on retry success) or `Failed` (on retry exhaust). This is a transient state — the caller does not need to take action.

### Requirement 3: Declarative Agent Schema

**User Story:** As a developer, I want to declare my agent as a JSON/Rust struct (model + system prompt + tools + MCP + skills + permissions), so that the runtime can build and run it without me managing the infrastructure.

#### Acceptance Criteria

1. THE module SHALL define a serializable `ManagedAgentDef` with fields: `model: ModelRef`, `system: Option<String>`, `tools: Vec<ToolConfig>`, `mcp_servers: Vec<McpServerConfig>`, `skills: Vec<SkillRef>`, `permission_policy: Option<PermissionPolicy>`, `metadata: Option<BTreeMap<String, String>>`.
2. THE runtime SHALL build a runnable agent (model + toolset + skill plugin + sandbox bindings) from a `ManagedAgentDef` deterministically.
3. `ManagedAgentDef` SHALL serialize to the CANON §3.1/§3.6–§3.9 wire shapes.
4. THE `ModelRef` type SHALL support: shorthand string (`"gemini-2.5-flash"`), structured object (`{provider: "openai", model: "gpt-4.1"}`), and openai_compatible with `{model, base_url, api_key}` where the key is provided resolved (never a ref — the platform resolves refs before passing to runtime).

### Requirement 4: Uniform Event Contract

**User Story:** As the platform team, I want one event taxonomy regardless of provider, so that SDKs and consumers see the same stream whether the agent uses Gemini or Claude.

#### Acceptance Criteria

1. THE runtime SHALL emit a provider-neutral `SessionEvent` stream with the CANON §3.4 variants (`agent.message`, `agent.tool_use`, `agent.custom_tool_use`, `agent.mcp_tool_use`, `status.running`, `status.idle`, `error`), each carrying a strictly monotonic `seq: u64` per session.
2. THE runtime SHALL accept the CANON §3.4 `UserEvent` variants (`user.message`, `user.interrupt`, `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result`, `user.define_outcome`). An unknown `type` SHALL be rejected with a typed error, not a panic.
3. WHEN the agent emits `agent.custom_tool_use`, THE runtime SHALL park the loop until the matching `user.custom_tool_result` arrives (matched by `custom_tool_use_id`). IF a configurable timeout elapses before the result arrives, THEN an error tool-result SHALL be surfaced to the loop so the agent can recover (no hang).
4. THE event stream SHALL be consumable as an async `Stream` and SHALL support replay from a given sequence ID. WHEN `stream_events(session_id, from_seq=Some(k))` is called, THEN exactly the events with `seq > k` SHALL be replayed, in order, exactly once.
5. WHEN `user.interrupt` is received mid-turn, THE runtime SHALL stop the loop at the next safe boundary and emit `status.idle`.
6. `user.tool_result` (returning a built-in tool result from the client) SHALL only be accepted in self-hosted topology. In hosted topology, built-in tools execute server-side in the sandbox.
7. WHEN the session emits `status.idle`, THE event SHALL include a `stop_reason` field with value `end_turn` (LLM naturally ended), `requires_action` (with `event_ids` of pending custom tool calls), or `max_tokens` (LLM hit token limit). This enables callers to determine the appropriate next action without inspecting preceding events.

### Requirement 5: Provider Parity

**User Story:** As a developer, I want to swap providers (Gemini → Claude → DeepSeek) without changing my integration code, knowing the event stream shape is identical.

#### Acceptance Criteria

1. Tool-calling SHALL work through the loop for all providers: gemini, openai, anthropic, ollama, and openai_compatible.
2. Tool/response JSON-Schema normalization SHALL be applied per provider so MCP tools with `$schema`/`additionalProperties`/nested `response` schemas are accepted and callable by every provider.
3. Streaming token output and usage metadata (input/output tokens) SHALL be reported uniformly so the platform can meter cost per provider.
4. `openai_compatible` SHALL accept `base_url` + provided key and handle provider tool-call dialect differences.
5. **Provider parity gate (F-8):** An identical `ManagedAgentDef` run against all five providers with mock `Llm` doubles SHALL produce byte-identical `SessionEvent` type sequences. This is the primary correctness gate.

### Requirement 6: Sandbox Integration

**User Story:** As a developer, I want my managed agent to have an isolated workspace for bash/filesystem tools, so that it operates safely without access to the host system.

#### Acceptance Criteria

1. THE runtime SHALL optionally attach a `SandboxRunner` so filesystem/shell tools bind to a live session sandbox. Path traversal (`..`/absolute paths) SHALL be rejected.
2. THE runtime SHALL support snapshot-on-stop and resume-from-snapshot, surfacing a snapshot ID for the platform to persist.
3. Combining a provider-managed sandbox (e.g., Gemini Interactions) with a client `SandboxRunner` SHALL be a build/startup-time error to prevent conflicts.

### Requirement 7: Telemetry

**User Story:** As the platform team, I want standardized telemetry spans from the runtime, so that I can export metrics and traces for monitoring dashboards.

#### Acceptance Criteria

1. THE runtime SHALL emit `adk-telemetry` spans around the loop, each tool call, and each provider call, with stable attributes: `session_id`, `model`, `provider`, `tool_name`.
2. THE runtime SHALL expose status via the `status()` method and the event stream (`stream_events`) for monitoring.

### Requirement 8: Memory Integration

**User Story:** As a developer, I want cross-session memory so my agent remembers context from previous sessions.

#### Acceptance Criteria

1. THE runtime SHALL optionally accept a pluggable `Memory` service (`adk-memory`) for cross-session persistent memory.
2. WHEN memory is configured, THE runtime SHALL make relevant memories available to the loop and persist new memories produced during a session.

### Requirement 9: ModelRef Resolution

**User Story:** As the runtime, I need to resolve a `ModelRef` into a live `Arc<dyn Llm>` so that any provider declaration becomes a callable model.

#### Acceptance Criteria

1. THE module SHALL provide a `resolve_model(model_ref: &ModelRef, credentials: &dyn CredentialProvider) -> Result<Arc<dyn Llm>>` function that constructs the appropriate model client.
2. FOR `openai_compatible`, the resolver SHALL accept `{model, base_url, api_key}` and construct an OpenAI-compatible client pointed at the given URL.
3. IF the provider is unsupported or the model cannot be constructed, THEN the resolver SHALL return a typed error.

### Requirement 10: Non-functional & Compatibility

**User Story:** As an existing ADK-Rust user, I want these additions to be purely additive, so that my code continues to compile and run without changes.

#### Acceptance Criteria

1. All new surface SHALL be additive and feature-gated (`managed-runtime`). No breaking change to `Runner`, `LlmAgent`, or existing `SessionService` implementations.
2. New public types SHALL be `#[non_exhaustive]` where polymorphic and derive `Debug, Clone, Serialize, Deserialize`.
3. THE runtime SHALL not depend on any platform crate (`ep-*`). Dependency direction is platform → adk-rust only.
4. THE module SHALL include property tests for: resume round-trip (R-2), event replay (R-4.4), and per-provider tool-call parity (R-5).
5. Golden fixture tests F-1 through F-8 SHALL pass as the conformance gate.
