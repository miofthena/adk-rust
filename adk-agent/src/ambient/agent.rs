use std::sync::Arc;

use adk_core::{AdkError, Agent, Result};
use futures::StreamExt;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;

use super::event_source::EventSource;

/// Lifecycle status of an [`AmbientAgent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbientAgentStatus {
    /// The agent is actively processing events.
    Running,
    /// The agent is paused — subscription is alive but events are buffered, not processed.
    Paused,
    /// The agent is stopped — no background task is running.
    Stopped,
}

/// A background agent triggered by an event source.
///
/// Wraps an [`Agent`] and an [`EventSource`], providing lifecycle control
/// (start, stop, pause, resume) over the background event processing loop.
///
/// # Lifecycle
///
/// ```text
/// Stopped → start() → Running → pause() → Paused → resume() → Running
///                        │                     │
///                        └── stop() → Stopped ←┘
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use adk_agent::ambient::{AmbientAgent, CronTrigger};
///
/// let trigger = CronTrigger::new("0 * * * * *")?;
/// let mut ambient = AmbientAgent::new(agent, Arc::new(trigger));
/// ambient.start().await?;
/// // ... later
/// ambient.stop().await?;
/// ```
pub struct AmbientAgent {
    agent: Arc<dyn Agent>,
    source: Arc<dyn EventSource>,
    status: Arc<RwLock<AmbientAgentStatus>>,
    resume_notify: Arc<Notify>,
    handle: Option<JoinHandle<()>>,
}

impl AmbientAgent {
    /// Create a new ambient agent wrapping the given agent and event source.
    ///
    /// The agent starts in [`AmbientAgentStatus::Stopped`] state.
    pub fn new(agent: Arc<dyn Agent>, source: Arc<dyn EventSource>) -> Self {
        Self {
            agent,
            source,
            status: Arc::new(RwLock::new(AmbientAgentStatus::Stopped)),
            resume_notify: Arc::new(Notify::new()),
            handle: None,
        }
    }

    /// Start listening for events and invoking the agent.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is already running or paused.
    pub async fn start(&mut self) -> Result<()> {
        let current = *self.status.read().await;
        if current != AmbientAgentStatus::Stopped {
            return Err(AdkError::agent("agent already running"));
        }

        // Subscribe to the event source
        let stream = self.source.subscribe().await?;

        let status = Arc::clone(&self.status);
        let resume_notify = Arc::clone(&self.resume_notify);
        let agent = Arc::clone(&self.agent);

        *self.status.write().await = AmbientAgentStatus::Running;

        let handle = tokio::spawn(async move {
            let mut stream = stream;

            while let Some(event) = stream.next().await {
                // Check if paused — wait until resumed
                loop {
                    let current_status = *status.read().await;
                    match current_status {
                        AmbientAgentStatus::Running => break,
                        AmbientAgentStatus::Paused => {
                            // Wait for resume signal
                            resume_notify.notified().await;
                        }
                        AmbientAgentStatus::Stopped => return,
                    }
                }

                // Process the event — log the trigger and note that agent invocation
                // would happen here. Without a Runner reference, we can't fully invoke
                // the agent, so we log the event details.
                tracing::info!(
                    agent = agent.name(),
                    source = %event.source,
                    "ambient agent triggered (agent invocation placeholder)"
                );
                tracing::debug!(payload = %event.payload, "trigger event payload");
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    /// Stop the agent and cancel in-progress work.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is already stopped.
    pub async fn stop(&mut self) -> Result<()> {
        let current = *self.status.read().await;
        if current == AmbientAgentStatus::Stopped {
            return Err(AdkError::agent("agent already stopped"));
        }

        *self.status.write().await = AmbientAgentStatus::Stopped;

        // Wake the task if paused so it can observe the Stopped state
        self.resume_notify.notify_one();

        if let Some(handle) = self.handle.take() {
            handle.abort();
        }

        Ok(())
    }

    /// Pause event processing. The subscription remains alive but events are buffered.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is not currently running.
    pub async fn pause(&mut self) -> Result<()> {
        let current = *self.status.read().await;
        if current != AmbientAgentStatus::Running {
            return Err(AdkError::agent("can only pause a running agent"));
        }

        *self.status.write().await = AmbientAgentStatus::Paused;
        Ok(())
    }

    /// Resume event processing after a pause.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is not currently paused.
    pub async fn resume(&mut self) -> Result<()> {
        let current = *self.status.read().await;
        if current != AmbientAgentStatus::Paused {
            return Err(AdkError::agent("can only resume a paused agent"));
        }

        *self.status.write().await = AmbientAgentStatus::Running;
        self.resume_notify.notify_one();
        Ok(())
    }

    /// Read the current lifecycle status.
    pub async fn status(&self) -> AmbientAgentStatus {
        *self.status.read().await
    }
}

impl Drop for AmbientAgent {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl std::fmt::Debug for AmbientAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmbientAgent")
            .field("agent", &self.agent.name())
            .field("source", &self.source.name())
            .finish()
    }
}
