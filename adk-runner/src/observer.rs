//! Runtime observation: a cheap, generic hook for building a lifecycle journal.
//!
//! The observer types now live in [`adk_core::observer`] so the lowest layers —
//! the `InvocationContext` trait accessor and the agent's model-call emit sites —
//! can name them. They are re-exported here (and at the crate root) so existing
//! callers of `adk_runner::observer::*` / `adk_runner::RunObserver` compile
//! unchanged.
//!
//! Register a single [`RunObserver`] on the
//! [`RunnerConfigBuilder::run_observer`](crate::RunnerConfigBuilder::run_observer)
//! to receive the run lifecycle (invocation queued/started/completed/failed/
//! cancelled, model call started/completed, and tool call started/completed)
//! **without** re-deriving it from the event stream. It is entirely opt-in and
//! additive: with no observer registered the runner does zero extra work, and
//! observer errors are logged and dropped, never failing the run.

pub use adk_core::observer::{RunObserver, RuntimeEvent, RuntimeEventKind};
