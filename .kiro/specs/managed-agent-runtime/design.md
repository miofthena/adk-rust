# Design Document: Managed Agent Runtime

## Overview

The Managed Agent Runtime is the provider-neutral execution engine for ADK-Rust Enterprise. It takes a declarative `ManagedAgentDef`, builds a runnable agent, and operates it as a durable, resumable, event-streaming background session. The runtime composes existing shipping components behind a unified lifecycle trait.

This document is the definitive architecture reference. It covers the three-layer system architecture, the session loop micro-flow, ownership boundaries, and how this design compares to alternative approaches (notably Anthropic's managed agents client).

### Design Goals

1. **Provider-neutral**: Identical event sequences regardless of model provider (fixture F-8 gate)
2. **Durable**: Checkpoint after every event; survive process crashes with zero event loss
3. **Resumable**: Rehydrate from checkpoint; continue from last consistent state
4. **Composable**: Injected services (sessions, sandbox, memory, telemetry) — no platform deps
5. **Wire-compatible**: Types serialize to CANON §3.4 shapes; SDKs generate from the same OpenAPI
6. **Additive**: Feature-gated; existing `Runner`/`LlmAgent` unchanged when feature is off

---

## Architecture

### Three-Layer Architecture

The managed agent system is split into three distinct layers with strict dependency direction (top → bottom only):

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        DEVELOPER SDK (future work)                               │
│                                                                                 │
│   ManagedAgentsClient (Rust/Python/TS)                                          │
│   ─────────────────────────────────────                                         │
│   • create_agent(def) → agent_id                                                │
│   • start_session(agent_id) → session_id                                        │
│   • send_message(session_id, content) → SSE stream                              │
│   • interrupt / pause / resume / archive                                        │
│   • Reconnect via Last-Event-ID header                                          │
│                                                                                 │
│   Looks identical to Anthropic's ManagedAgentsClient.                           │
│   The difference: ours talks to OUR platform, not theirs.                       │
└──────────────────────────────────┬──────────────────────────────────────────────┘
                                   │ HTTP/SSE
                                   ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        PLATFORM LAYER (ep-* crates)                              │
│                                                                                 │
│   HTTP Routes │ Auth + RBAC │ Billing │ Multi-tenancy │ ID Prefixes             │
│   ─────────────────────────────────────────────────────────────────             │
│   • Maps HTTP POST /sessions/{id}/events → runtime.send_event()                 │
│   • Maps SSE GET /sessions/{id}/events → runtime.stream_events()                │
│   • Maps Last-Event-ID header → from_seq parameter                              │
│   • Resolves credential refs (vault) → plaintext keys for runtime               │
│   • Assigns agt_/ses_ prefixed IDs                                              │
│   • Enforces tenant isolation, rate limits, billing meters                       │
│   • NO execution logic — pure routing + governance                              │
│                                                                                 │
└──────────────────────────────────┬──────────────────────────────────────────────┘
                                   │ Rust trait calls (in-process)
                                   ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                      RUNTIME LAYER (adk-managed — THIS DELIVERABLE)              │
│                                                                                 │
│   ManagedAgentRuntime trait + DefaultManagedAgentRuntime                         │
│   ─────────────────────────────────────────────────────                         │
│   • Builds runnable agents from ManagedAgentDef                                 │
│   • Runs the supervised session loop (durable, resumable)                       │
│   • Emits provider-neutral SessionEvent stream                                  │
│   • Manages custom tool parking, checkpoints, interrupts                        │
│   • Resolves ModelRef → Arc<dyn Llm>                                            │
│   • Zero platform dependencies                                                  │
│                                                                                 │
│   Composes existing shipping crates:                                            │
│   ┌──────────┐ ┌──────────────┐ ┌───────────┐ ┌──────────┐ ┌────────────┐     │
│   │adk-runner│ │adk-session   │ │adk-model  │ │adk-tool  │ │adk-sandbox │     │
│   │          │ │(SessionSvc)  │ │(all provs)│ │(MCP,etc) │ │(optional)  │     │
│   └──────────┘ └──────────────┘ └───────────┘ └──────────┘ └────────────┘     │
│   ┌──────────┐ ┌──────────────┐ ┌───────────┐                                  │
│   │adk-skill │ │adk-memory    │ │adk-telem  │                                  │
│   │          │ │(optional)    │ │           │                                  │
│   └──────────┘ └──────────────┘ └───────────┘                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Why This Layering Matters

The runtime is a **library**, not a service. The platform hosts it. The SDK talks to the platform. This means:

- **Testable in isolation**: The runtime has zero HTTP/auth/billing dependencies
- **Embeddable**: Self-hosted deployments use the same runtime trait directly (no HTTP hop)
- **Swappable platform**: Different platforms (cloud, on-prem, edge) can host the same runtime
- **Future multiagent**: Thread-awareness is a runtime concern (session-per-thread model)

---

## Comparison with Anthropic's Architecture

Understanding what Anthropic ships publicly vs. what they keep internal clarifies our design:

| Aspect | Anthropic (open-source) | ADK-Rust (this project) |
|--------|------------------------|------------------------|
| **What's published** | `managed_agents` module: ~1300 lines, thin HTTP client | `adk-managed` crate: full runtime engine |
| **What it does** | Formats HTTP requests, parses SSE responses, reconnects on `Last-Event-ID` | Builds agents, runs the loop, checkpoints, parks, resumes |
| **Session loop** | Lives inside Anthropic's cloud (not open-sourced) | Lives HERE — we ARE the session loop |
| **Tool execution** | Anthropic's servers execute tools in their sandbox | Our runtime executes tools (or parks for client-executed custom tools) |
| **Checkpointing** | Anthropic's internal infrastructure | Our `CheckpointManager` + pluggable `SessionService` |
| **Provider support** | Claude only (naturally) | Gemini, OpenAI, Anthropic, Ollama, OpenAI-compatible |
| **SDK surface** | `ManagedAgentsClient` (Python) | Future: identical-looking SDK, but talks to OUR platform |

**Key insight**: Anthropic's `managed_agents` module is a *client OF their service*. Our `adk-managed` crate *IS the service* (the black box they don't open-source). When we build our SDK later, it will look nearly identical to `ManagedAgentsClient` — but it talks to our platform, which hosts our runtime.

This is why `adk-managed` is architecturally significant: it's the execution engine, not a wrapper.

---

## Ownership Boundary Table

| Concern | adk-managed (runtime) | Platform (ep-*) | Existing crates |
|---------|----------------------|-----------------|-----------------|
| Session loop execution | ✅ owns | — | Runner provides per-turn execution |
| Checkpoint persistence | ✅ orchestrates | — | SessionService provides storage |
| Custom tool parking | ✅ owns | — | — |
| Event sequence assignment | ✅ owns | — | — |
| Event replay (from_seq) | ✅ owns | Maps Last-Event-ID header | — |
| ModelRef resolution | ✅ owns | Resolves credential refs first | adk-model provides constructors |
| Agent building from def | ✅ owns | — | LlmAgent, McpToolset, etc. |
| Interrupt/pause/resume | ✅ owns | Routes HTTP to trait method | — |
| Provider-neutral mapping | ✅ owns | — | Runner emits provider-specific events |
| Multi-tenancy | — | ✅ owns | — |
| RBAC + auth | — | ✅ owns | — |
| Billing + metering | — | ✅ owns | — |
| HTTP/SSE routes | — | ✅ owns | — |
| ID generation (agt_, ses_) | — | ✅ owns | — |
| Vault/secret resolution | — | ✅ owns | — |
| LLM API calls | — | — | ✅ adk-model |
| Tool registration + exec | — | — | ✅ adk-tool |
| MCP lifecycle | — | — | ✅ adk-tool (McpToolset) |
| Sandbox isolation | — | — | ✅ adk-sandbox |
| Session persistence impl | — | — | ✅ adk-session |
| Telemetry spans | — | — | ✅ adk-telemetry |
| Memory service | — | — | ✅ adk-memory |

---

## Session Loop Detailed Flow

The supervised session loop is the core execution engine. This diagram shows the exact micro-flow including event processing, tool calling, parking, and checkpointing:

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│  Session Loop (tokio::spawn, one per active session)                            │
│                                                                                 │
│  INITIALIZATION:                                                                │
│  ┌──────────────────────────────────────────────────────────────────┐           │
│  │ • Status: Queued → Running (on first event dequeue)              │           │
│  │ • Load checkpoint (if resuming): events + RunState               │           │
│  │ • Attach to broadcast channel for stream subscribers             │           │
│  │ • Initialize SequenceCounter (from checkpoint seq or 0)          │           │
│  └──────────────────────────────────────────────────────────────────┘           │
│                                                                                 │
│  MAIN LOOP:                                                                     │
│  ┌──────────────────────────────────────────────────────────────────┐           │
│  │ 1. DEQUEUE UserEvent from input channel (mpsc)                   │           │
│  │    ├── user.message → proceed to step 2                          │           │
│  │    ├── user.interrupt → signal CancellationToken, goto 7         │           │
│  │    ├── user.custom_tool_result → deliver to parking lot          │           │
│  │    ├── user.tool_confirmation → deliver to confirmation handler  │           │
│  │    └── user.define_outcome → store criteria                      │           │
│  │                                                                  │           │
│  │ 2. EMIT SessionEvent::StatusRunning { seq: next() }              │           │
│  │    • Checkpoint atomically                                       │           │
│  │    • Broadcast to subscribers                                    │           │
│  │                                                                  │           │
│  │ 3. INVOKE Runner.run() with user content                         │           │
│  │    • Runner drives the LLM + tool loop                           │           │
│  │    • Returns an event stream                                     │           │
│  │                                                                  │           │
│  │ 4. FOR EACH event from Runner:                                   │           │
│  │    a. MAP to SessionEvent variant:                               │           │
│  │       • LLM text → agent.message                                 │           │
│  │       • Function call (built-in) → agent.tool_use                │           │
│  │       • Function call (custom) → agent.custom_tool_use           │           │
│  │       • Function call (MCP) → agent.mcp_tool_use                 │           │
│  │    b. ASSIGN seq: next() (strictly monotonic)                    │           │
│  │    c. CHECKPOINT atomically (event + RunState)                   │           │
│  │    d. BROADCAST to stream subscribers                            │           │
│  │    e. IF agent.custom_tool_use:                                  │           │
│  │       ┌─────────────────────────────────────────────┐            │           │
│  │       │ PARK: ToolParkingLot.park(custom_tool_use_id)│           │           │
│  │       │ • Wait for user.custom_tool_result           │           │           │
│  │       │ • OR timeout → inject error tool result      │           │
│  │       │ • Feed result back to Runner                 │           │           │
│  │       └─────────────────────────────────────────────┘            │           │
│  │    f. IF tool_confirmation required:                             │           │
│  │       ┌─────────────────────────────────────────────┐            │           │
│  │       │ PARK: Wait for user.tool_confirmation        │           │           │
│  │       │ • Allow → continue execution                 │           │
│  │       │ • Deny → inject deny result to Runner        │           │           │
│  │       └─────────────────────────────────────────────┘            │           │
│  │    g. CHECK CancellationToken (interrupt boundary)               │           │
│  │    h. CHECK pause flag (pause boundary)                          │           │
│  │                                                                  │           │
│  │ 5. TURN COMPLETE — determine stop reason:                        │           │
│  │    • end_turn: LLM naturally ended                               │           │
│  │    • requires_action: custom tools still pending                 │           │
│  │    • max_tokens: LLM hit token limit                             │           │
│  │                                                                  │           │
│  │ 6. EMIT SessionEvent::StatusIdle { seq: next(), stop_reason }    │           │
│  │    • Checkpoint atomically                                       │           │
│  │    • Broadcast to subscribers                                    │           │
│  │    • Status: Running → Idle                                      │           │
│  │                                                                  │           │
│  │ 7. LOOP back to step 1                                           │           │
│  └──────────────────────────────────────────────────────────────────┘           │
│                                                                                 │
│  INTERRUPT HANDLING:                                                            │
│  • CancellationToken checked between Runner events (step 4g)                    │
│  • On interrupt: stop at boundary, emit status.idle, status → Idle              │
│                                                                                 │
│  PAUSE HANDLING:                                                                │
│  • Pause flag checked after each event (step 4h)                                │
│  • On pause: checkpoint, stop dequeuing, status → Paused                        │
│  • On resume: clear flag, notify loop, status → Running                         │
│                                                                                 │
│  CRASH RECOVERY:                                                                │
│  • On restart: resume() loads last checkpoint                                   │
│  • Replay committed events to subscribers                                       │
│  • Re-enter at step 1 with correct seq counter                                  │
│                                                                                 │
│  TRANSIENT ERROR HANDLING:                                                      │
│  • On retryable provider error: status → Rescheduling                           │
│  • Auto-retry with backoff (configurable)                                       │
│  • On success: status → Running, continue                                       │
│  • On exhaust: status → Failed                                                  │
│                                                                                 │
│  THREAD-AWARENESS NOTE (future multiagent):                                     │
│  • Current: one session = one thread of execution                               │
│  • Future: session may spawn child sessions (sub-agents)                        │
│  • The loop is designed to be one-per-thread, composable via orchestration      │
│  • No shared mutable state between loops — coordination via events only         │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Module Structure

```
adk-managed/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # Feature gate, exports
│   ├── runtime.rs              # ManagedAgentRuntime trait
│   ├── default_runtime.rs      # DefaultManagedAgentRuntime implementation
│   ├── types/
│   │   ├── mod.rs              # Re-exports
│   │   ├── agent_def.rs        # ManagedAgentDef, ModelRef, ToolConfig, etc.
│   │   ├── events.rs           # UserEvent, SessionEvent, ContentBlock, StopReason
│   │   ├── session.rs          # SessionStatus, SessionInfo, Usage
│   │   └── error.rs            # RuntimeError enum
│   ├── resolver.rs             # ModelRef → Arc<dyn Llm>
│   ├── agent_builder.rs        # ManagedAgentDef → runnable agent
│   ├── session_loop.rs         # Supervised loop: run turns, park, checkpoint
│   ├── parking.rs              # Custom tool parking (channel-based wait)
│   ├── checkpoint.rs           # Atomic event+state persistence
│   └── sequence.rs             # Monotonic seq counter per session
└── tests/
    ├── fixtures/               # Golden fixture JSONs (F-1 through F-8)
    │   ├── README.md
    │   ├── f1_hello.json
    │   ├── f2_mcp_tool.json
    │   ├── f3_custom_tool.json
    │   ├── f4_confirmation.json
    │   ├── f5_resume.json
    │   ├── f6_replay.json
    │   ├── f7_interrupt.json
    │   └── f8_provider_parity.json
    ├── runtime_lifecycle_tests.rs
    ├── durability_tests.rs
    ├── event_contract_tests.rs
    ├── provider_parity_tests.rs
    └── fixture_conformance_tests.rs
```

---

## Components and Interfaces

### ManagedAgentRuntime Trait

```rust
/// The central lifecycle trait for managed agent execution.
///
/// Implementations orchestrate the full agent lifecycle: creation from a
/// declarative definition, session management (start/pause/resume/stop),
/// event dispatch, and durable checkpoint/resume.
///
/// The platform hosts this trait behind HTTP routes; the runtime handles
/// the execution mechanics.
#[async_trait]
pub trait ManagedAgentRuntime: Send + Sync {
    /// Register an agent definition. Returns a runtime-internal handle.
    /// The platform assigns the `agt_` prefixed ID; the runtime uses an opaque handle.
    async fn create(&self, def: ManagedAgentDef) -> Result<AgentHandle, RuntimeError>;

    /// Start a new session for the given agent. Initial status is `Queued`.
    async fn start_session(
        &self,
        agent: &AgentHandle,
        env: Option<EnvironmentConfig>,
    ) -> Result<SessionHandle, RuntimeError>;

    /// Send a UserEvent to a session. Enqueues for processing by the loop.
    async fn send_event(
        &self,
        session: &SessionHandle,
        event: UserEvent,
    ) -> Result<(), RuntimeError>;

    /// Subscribe to the session event stream.
    /// `from_seq = None` → live tail only.
    /// `from_seq = Some(k)` → replay all events with seq > k, then live tail.
    ///
    /// Note: The platform maps the HTTP `Last-Event-ID` header to `from_seq`.
    /// This is a platform concern — the runtime only exposes the sequence-based API.
    async fn stream_events(
        &self,
        session: &SessionHandle,
        from_seq: Option<u64>,
    ) -> Result<BoxStream<'static, SessionEvent>, RuntimeError>;

    /// Interrupt the current turn. Loop stops at next boundary, emits `status.idle`.
    async fn interrupt(&self, session: &SessionHandle) -> Result<(), RuntimeError>;

    /// Pause: stop consuming input, checkpoint. Session status → `Paused`.
    async fn pause(&self, session: &SessionHandle) -> Result<(), RuntimeError>;

    /// Resume from pause or from a process restart. Status → `Running`/`Idle`.
    async fn resume(&self, session: &SessionHandle) -> Result<(), RuntimeError>;

    /// Query current session status.
    async fn status(&self, session: &SessionHandle) -> Result<SessionStatus, RuntimeError>;

    /// Archive a session (terminal state, data retained for read).
    async fn archive(&self, session: &SessionHandle) -> Result<(), RuntimeError>;

    /// Delete a session (data removed).
    async fn delete_session(&self, session: &SessionHandle) -> Result<(), RuntimeError>;
}
```

### DefaultManagedAgentRuntime

```rust
/// Default implementation composed from existing shipping components.
/// No platform dependencies — accepts injected services.
pub struct DefaultManagedAgentRuntime {
    /// Builds `Arc<dyn Llm>` from `ModelRef` + credentials.
    model_resolver: Arc<dyn ModelResolver>,
    /// Session persistence (platform supplies Postgres; tests use InMemory).
    session_service: Arc<dyn SessionService>,
    /// Optional sandbox for filesystem/shell tools.
    sandbox_factory: Option<Arc<dyn SandboxFactory>>,
    /// Optional cross-session memory.
    memory: Option<Arc<dyn MemoryService>>,
    /// Active sessions: SessionHandle → supervised loop handle.
    sessions: Arc<RwLock<HashMap<String, ActiveSession>>>,
}
```

---

## Data Models

### Wire Types

### UserEvent (Client → Agent)

```rust
/// Client-to-agent event. Discriminated by `type` field for wire serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UserEvent {
    /// Send a message turn.
    #[serde(rename = "user.message")]
    Message { content: Vec<ContentBlock> },

    /// Interrupt the current turn.
    #[serde(rename = "user.interrupt")]
    Interrupt {},

    /// Approve or deny a tool confirmation request.
    #[serde(rename = "user.tool_confirmation")]
    ToolConfirmation {
        tool_use_id: String,
        result: ConfirmationResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        deny_message: Option<String>,
    },

    /// Return results for a client-executed custom tool.
    #[serde(rename = "user.custom_tool_result")]
    CustomToolResult {
        custom_tool_use_id: String,
        content: Vec<ContentBlock>,
    },

    /// Return results for a built-in tool (self-hosted only).
    /// In hosted topology, built-in tools execute server-side in the sandbox.
    /// The platform enforces this constraint; the runtime accepts the event unconditionally.
    #[serde(rename = "user.tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
    },

    /// Define success criteria for the session.
    #[serde(rename = "user.define_outcome")]
    DefineOutcome { criteria: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmationResult {
    Allow,
    Deny,
}
```

### SessionEvent (Agent → Client)

```rust
/// Agent-to-client event. Each carries a monotonic `seq` per session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    /// Assistant message content.
    #[serde(rename = "agent.message")]
    Message { content: Vec<ContentBlock>, seq: u64 },

    /// Built-in tool invocation (executes server-side in sandbox).
    #[serde(rename = "agent.tool_use")]
    ToolUse {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        seq: u64,
    },

    /// Custom tool invocation (client must execute and return result).
    /// The loop PARKS until `user.custom_tool_result` with matching ID arrives.
    #[serde(rename = "agent.custom_tool_use")]
    CustomToolUse {
        custom_tool_use_id: String,
        name: String,
        input: serde_json::Value,
        seq: u64,
    },

    /// MCP tool invocation.
    #[serde(rename = "agent.mcp_tool_use")]
    McpToolUse {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        seq: u64,
    },

    /// Session became active (processing a turn).
    #[serde(rename = "status.running")]
    StatusRunning { seq: u64 },

    /// Turn complete; awaiting next event.
    /// Includes `stop_reason` to tell the caller WHY the turn ended.
    #[serde(rename = "status.idle")]
    StatusIdle {
        seq: u64,
        /// Why the turn ended. Enables the client to decide what to do next.
        stop_reason: Option<StopReason>,
    },

    /// Error during execution.
    #[serde(rename = "error")]
    Error { code: String, message: String, seq: u64 },
}

/// Why a turn ended. Included in `status.idle` events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The LLM naturally ended its turn (end_turn stop reason from provider).
    EndTurn,
    /// The agent emitted custom tool calls that require client action.
    /// The caller must send `user.custom_tool_result` for each listed event_id.
    RequiresAction { event_ids: Vec<String> },
    /// The LLM hit its maximum token limit mid-generation.
    MaxTokens,
}
```

### ContentBlock

```rust
/// Content within a message. Forward-compatible: unknown types are opaque.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text content.
    Text { text: String },
    /// Image content.
    Image { source: serde_json::Value },
    /// File reference.
    File { file_id: String },
}
```

### ManagedAgentDef

```rust
/// Declarative agent definition. Serializes to CANON §3.1/§3.6–§3.9.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub struct ManagedAgentDef {
    /// Human-readable agent name.
    pub name: String,
    /// Provider-neutral model reference.
    pub model: ModelRef,
    /// System prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Agent description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tool declarations (built-in + custom).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolConfig>,
    /// MCP server configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Skill references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillRef>,
    /// Permission policy for tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
    /// Caller metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}
```

### ModelRef

```rust
/// Provider-neutral model reference (CANON §3.6).
///
/// Either a shorthand string or a structured provider+model object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelRef {
    /// Shorthand: just the model name (e.g., "gemini-2.5-flash").
    Shorthand(String),
    /// Structured: explicit provider and model configuration.
    Structured {
        provider: Provider,
        model: ModelConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        speed: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gemini,
    Openai,
    Anthropic,
    Ollama,
    OpenaiCompatible,
}

/// Model configuration. For `openai_compatible`, includes base_url and api_key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelConfig {
    /// Simple model name string.
    Name(String),
    /// OpenAI-compatible with custom endpoint.
    Compatible {
        model: String,
        base_url: String,
        /// The resolved API key (platform resolves credential refs before passing to runtime).
        api_key: String,
    },
}
```

### SessionStatus

```rust
/// Session lifecycle state. Entry state is `Queued`.
///
/// State machine:
/// ```text
/// Queued → Running → Idle (per turn) → Running (next turn)
///                  → Rescheduling → Running (on retry success)
///                  → Rescheduling → Failed (on retry exhaust)
///                  → Paused → Running (on resume)
///                  → Completed / Failed / Archived
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SessionStatus {
    /// Session created, waiting for first event to process.
    Queued,
    /// Actively processing a turn.
    Running,
    /// Turn complete, awaiting next input.
    Idle,
    /// Transient error encountered; auto-retrying with backoff.
    /// Transitions: Running → Rescheduling → Running (success) | Failed (exhaust).
    Rescheduling,
    /// Explicitly paused by caller; checkpoint saved.
    Paused,
    /// Session completed successfully (terminal).
    Completed,
    /// Session failed (terminal).
    Failed,
    /// Session archived (terminal, data retained for read).
    Archived,
}
```

---

## Internal Components

### Custom Tool Parking

```rust
/// Manages parking/unparking for custom tool round-trips.
pub struct ToolParkingLot {
    /// Pending custom tool calls: tool_use_id → oneshot sender for result.
    pending: Arc<RwLock<HashMap<String, oneshot::Sender<Vec<ContentBlock>>>>>,
    /// Configurable timeout for pending tool calls.
    timeout: Duration,
}

impl ToolParkingLot {
    /// Park the loop waiting for a custom tool result.
    /// Returns the result content or a timeout error.
    pub async fn park(&self, tool_use_id: &str) -> Result<Vec<ContentBlock>, RuntimeError>;

    /// Deliver a result for a parked custom tool call.
    pub async fn deliver(&self, tool_use_id: &str, content: Vec<ContentBlock>) -> Result<(), RuntimeError>;
}
```

### Checkpoint Manager

```rust
/// Manages atomic checkpoint persistence.
/// Ensures event emission + state persistence happen atomically.
pub struct CheckpointManager {
    session_service: Arc<dyn SessionService>,
}

impl CheckpointManager {
    /// Atomically persist an event AND update session run-state.
    /// If either fails, neither is committed (transactional guarantee).
    pub async fn checkpoint(
        &self,
        session_id: &str,
        event: &SessionEvent,
        run_state: &RunState,
    ) -> Result<(), RuntimeError>;

    /// Load the last consistent checkpoint for resume.
    pub async fn load_checkpoint(
        &self,
        session_id: &str,
    ) -> Result<Option<(Vec<SessionEvent>, RunState)>, RuntimeError>;
}

/// Run state persisted at each checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    /// Current sequence number.
    pub seq: u64,
    /// IDs of custom tool calls currently parked (waiting for client response).
    pub pending_parking_ids: Vec<String>,
    /// Current session status.
    pub status: SessionStatus,
}
```

### ModelRef Resolver

```rust
/// Resolves a ModelRef into a live LLM client.
#[async_trait]
pub trait ModelResolver: Send + Sync {
    async fn resolve(&self, model_ref: &ModelRef) -> Result<Arc<dyn Llm>, RuntimeError>;
}

/// Default resolver using adk-model constructors.
/// Infers provider from model name prefix for shorthand refs.
pub struct DefaultModelResolver;
```

### Sequence Counter

```rust
/// Thread-safe monotonic sequence counter.
/// One per session, starts at 0 (or last checkpointed seq on resume).
pub struct SequenceCounter {
    value: AtomicU64,
}

impl SequenceCounter {
    pub fn new(start: u64) -> Self;
    pub fn next(&self) -> u64;
}
```

---

## Error Handling

```rust
/// Runtime errors aligned with CANON §5 error model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeError {
    #[error("invalid request: {message}")]
    InvalidRequest { message: String, param: Option<String> },

    #[error("session not found: {session_id}")]
    NotFound { session_id: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("provider error ({provider}): {message}")]
    ProviderError { provider: String, message: String },

    #[error("tool timeout: {tool_use_id} after {timeout_secs}s")]
    ToolTimeout { tool_use_id: String, timeout_secs: u64 },

    #[error("checkpoint failed: {message}")]
    CheckpointFailed { message: String },

    #[error("sandbox error: {message}")]
    SandboxError { message: String },

    #[error("internal error: {message}")]
    Internal { message: String },
}
```

---

## Correctness Properties

### Property 1: Event Sequence Monotonicity
*For any* session, all emitted `SessionEvent`s SHALL have strictly increasing `seq` values with no gaps.
**Validates: Requirements 4.1**

### Property 2: Checkpoint Atomicity
*For any* session, if a crash occurs at any point, the post-resume event log SHALL be a prefix-consistent continuation: no gap, no duplicate, no event emitted without a matching checkpoint.
**Validates: Requirements 2.2, 2.5**

### Property 3: Resume Consistency
*For any* session with events up to seq=N that has been checkpointed, resuming after a process kill SHALL produce no event with seq≤N and no committed event lost.
**Validates: Requirements 2.3**

### Property 4: Custom Tool Parking
*For any* `agent.custom_tool_use` emission, the loop SHALL park until either a matching `user.custom_tool_result` is received OR the timeout elapses (no deadlock, no dropped tool call).
**Validates: Requirements 4.3**

### Property 5: Provider Parity (THE PRIMARY GATE)
*For any* `ManagedAgentDef` and any fixed `Llm` mock response sequence, running the same def against all five provider adapters SHALL produce byte-identical `SessionEvent.type` sequences.
**Validates: Requirements 5.5**

### Property 6: Event Replay Completeness
*For any* stream request with `from_seq=k`, the returned stream SHALL contain exactly the events with seq>k, in order, exactly once.
**Validates: Requirements 4.4**

### Property 7: Session State Machine
*For any* session, status transitions SHALL follow the defined state machine. Invalid transitions SHALL return `RuntimeError::Conflict`. The `Rescheduling` state is transient and auto-resolves.
**Validates: Requirements 2.6, 2.7**

---

## Testing Strategy

### Golden Fixtures (F-1 through F-8)

Each fixture is a JSON file specifying:
```json
{
  "name": "F-1 Hello (no tools)",
  "agent_def": { "model": "mock", "system": "be brief" },
  "mock_responses": [{"content": [{"type": "text", "text": "pong"}]}],
  "user_events": [{"type": "user.message", "content": [{"type": "text", "text": "ping"}]}],
  "expect_sequence": ["status.running", "agent.message", "status.idle"]
}
```

| Fixture | Tests | Requirements |
|---------|-------|-------------|
| F-1 Hello | Basic lifecycle, no tools | R-1, R-4.1 |
| F-2 MCP tool | MCP tool invocation + schema normalization | R-5.2 |
| F-3 Custom tool | Parking + round-trip | R-4.3 |
| F-4 Confirmation | Permission policy + deny | R-4.2 |
| F-5 Resume | Kill + resume from checkpoint | R-2.3 |
| F-6 Replay | stream_events(from_seq) | R-4.4 |
| F-7 Interrupt | Mid-turn interrupt | R-4.5 |
| F-8 Provider parity | Same def, all providers, identical sequences | R-5.5 |

### Property Tests (proptest, 100+ iterations)

| # | Property | What it proves |
|---|----------|---------------|
| 1 | Seq monotonicity | P-1 |
| 2 | Checkpoint atomicity | P-2 |
| 3 | Resume no-gap no-dup | P-3 |
| 4 | Parking timeout | P-4 |
| 5 | Provider parity | P-5 |
| 6 | Replay completeness | P-6 |
| 7 | State machine validity | P-7 |

---

## Feature Flag

```toml
# In adk-managed/Cargo.toml (new crate)
[features]
default = []
# No features needed — the crate IS the feature.
# Consumers enable it by depending on adk-managed.

# In adk-rust/Cargo.toml (umbrella)
managed-runtime = ["dep:adk-managed"]
```

## Crate Dependencies (adk-managed)

```toml
[dependencies]
adk-core = { workspace = true }
adk-agent = { workspace = true }
adk-model = { workspace = true, features = ["gemini", "openai", "anthropic", "ollama"] }
adk-runner = { workspace = true }
adk-session = { workspace = true }
adk-tool = { workspace = true }
adk-skill = { workspace = true }
adk-sandbox = { workspace = true, optional = true }
adk-memory = { workspace = true, optional = true }
adk-telemetry = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
futures = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
```

---

## Design Decisions and Rationale

### Why `stream_events(from_seq)` Instead of `Last-Event-ID`

`Last-Event-ID` is an HTTP header defined in the SSE specification. It's a transport-layer concern. The runtime operates below the transport layer — it doesn't know about HTTP. The platform maps the header to the `from_seq` parameter:

```
Client: GET /sessions/{id}/events (Last-Event-ID: 42)
Platform: runtime.stream_events(session, from_seq=Some(42))
Runtime: returns events with seq > 42
```

This keeps the runtime testable without HTTP and allows non-HTTP transports (gRPC, WebSocket) to use the same replay mechanism.

### Why Initial Status is `Queued` (Not `Idle`)

A newly created session has never processed any input. `Idle` implies "finished a turn, waiting for next input." `Queued` correctly communicates "created, waiting to start." The state machine is:

```
Queued → (first send_event) → Running → Idle → Running → ...
```

### Why `user.tool_result` is Self-Hosted Only

In hosted topology, built-in tools (bash, filesystem, web_search) execute inside the sandbox server-side. The client never sees them execute. In self-hosted topology, there's no sandbox — the client runs tools locally and returns results. The platform enforces this topology constraint; the runtime accepts both events unconditionally (the platform gates `user.tool_result` in hosted mode).

### Thread-Awareness for Future Multiagent

The current design runs one session loop per session. For future multiagent orchestration:
- A parent session can spawn child sessions (sub-agent threads)
- Each child has its own loop, own seq counter, own checkpoint
- Coordination happens via events (parent sends to child, child emits back to parent)
- No shared mutable state between loops — this is intentional for crash isolation

The `SessionHandle` is already opaque enough to support this without breaking the trait.
