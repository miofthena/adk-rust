//! Runtime observation: a cheap, generic hook for building a lifecycle journal.
//!
//! A downstream can register a single [`RunObserver`] on the runner
//! builder to receive a stream of [`RuntimeEvent`]s describing the run lifecycle
//! (invocation queued/started/completed/failed/cancelled, model call
//! started/completed, and tool call started/completed) **without** re-deriving
//! that lifecycle from the event stream itself.
//!
//! The observer is entirely opt-in and additive:
//! - When no observer is registered the runtime does **zero** extra work.
//! - Observer errors never fail or block the run — they are logged and dropped.
//! - Every [`RuntimeEvent`] carries only **bounded** metadata and an optional
//!   `payload_ref` (a short hash/handle), never the payload, secrets, or PII.
//!
//! The types live in `adk-core` (rather than `adk-runner`) so the lowest layers
//! — the `InvocationContext` trait accessor and the agent's model-call emit
//! sites — can name them; `adk-runner` re-exports them at its historical paths
//! (`adk_runner::observer::*`, `adk_runner::RunObserver`, …) so existing callers
//! compile unchanged.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The lifecycle phase a [`RuntimeEvent`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    /// The invocation was accepted and registered, before it began executing.
    InvocationQueued,
    /// The invocation began executing the agent.
    InvocationStarted,
    /// A model call started.
    ModelCallStarted,
    /// A model call completed.
    ModelCallCompleted,
    /// A tool call started.
    ToolCallStarted,
    /// A tool call completed.
    ToolCallCompleted,
    /// The invocation finished successfully.
    InvocationCompleted,
    /// The invocation ended with an error.
    InvocationFailed,
    /// The invocation was cancelled (interrupted or its stream dropped).
    InvocationCancelled,
}

/// A single, bounded record of a runtime lifecycle transition.
///
/// Cheap to construct and clone: all payloads are reduced to sizes, counts, and
/// hashes. `metadata` and `payload_ref` never carry prompt text, tool
/// arguments/results, secrets, or PII.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    /// Which lifecycle phase this event reports.
    pub kind: RuntimeEventKind,
    /// The invocation that produced this event.
    pub invocation_id: String,
    /// The session the invocation belongs to.
    pub session_id: String,
    /// The agent associated with this phase (best-effort at pre-start phases).
    pub agent_name: String,
    /// A per-run, monotonically increasing sequence number (starts at 0).
    pub sequence: u64,
    /// When the event was observed.
    pub timestamp: DateTime<Utc>,
    /// Bounded, PII-free key/value metadata (e.g. `tool`, sizes/counts).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// An optional short handle/hash of an associated payload — never the
    /// payload itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<String>,
}

impl RuntimeEvent {
    /// Construct a minimal event; enrich via [`with_metadata`](Self::with_metadata)
    /// / [`with_payload_ref`](Self::with_payload_ref).
    pub fn new(
        kind: RuntimeEventKind,
        invocation_id: impl Into<String>,
        session_id: impl Into<String>,
        agent_name: impl Into<String>,
        sequence: u64,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            kind,
            invocation_id: invocation_id.into(),
            session_id: session_id.into(),
            agent_name: agent_name.into(),
            sequence,
            timestamp,
            metadata: BTreeMap::new(),
            payload_ref: None,
        }
    }

    /// Attach one bounded metadata entry.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Attach a short payload handle/hash (never the payload).
    pub fn with_payload_ref(mut self, payload_ref: impl Into<String>) -> Self {
        self.payload_ref = Some(payload_ref.into());
        self
    }
}

/// A cheap, generic observer of the runtime lifecycle.
///
/// Register one via `RunnerConfigBuilder::run_observer`.
/// `on_event` is awaited inline at each lifecycle point, so keep it fast (buffer
/// or spawn for heavy work). Returning an error does **not** fail the run — the
/// runtime logs it and continues.
#[async_trait]
pub trait RunObserver: Send + Sync {
    /// Handle a single lifecycle event. Errors are logged, never propagated.
    async fn on_event(&self, event: RuntimeEvent) -> crate::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_event_builder_and_serde_roundtrip() {
        let ts = Utc::now();
        let event = RuntimeEvent::new(
            RuntimeEventKind::ToolCallStarted,
            "inv-1",
            "sess-1",
            "coordinator",
            3,
            ts,
        )
        .with_metadata("tool", "bash")
        .with_payload_ref("abc123");

        assert_eq!(event.sequence, 3);
        assert_eq!(event.metadata.get("tool").map(String::as_str), Some("bash"));
        assert_eq!(event.payload_ref.as_deref(), Some("abc123"));

        let json = serde_json::to_string(&event).expect("serialize");
        let back: RuntimeEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.kind, RuntimeEventKind::ToolCallStarted);
        assert_eq!(back.invocation_id, "inv-1");
        assert_eq!(back.session_id, "sess-1");
    }

    #[test]
    fn model_call_kinds_serialize_snake_case() {
        // The model-call phases are part of the public vocabulary and must
        // round-trip through serde with stable snake_case names.
        for (kind, name) in [
            (RuntimeEventKind::ModelCallStarted, "\"model_call_started\""),
            (RuntimeEventKind::ModelCallCompleted, "\"model_call_completed\""),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), name);
            let back: RuntimeEventKind = serde_json::from_str(name).unwrap();
            assert_eq!(back, kind);
        }
    }
}
