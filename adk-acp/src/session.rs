//! Persistent ACP session with connection reuse.
//!
//! Unlike [`prompt_agent`](crate::prompt_agent) which spawns a fresh process per call,
//! [`AcpSession`] keeps the agent process alive across multiple prompts — preserving
//! context, reducing latency, and enabling session-based workflows.
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_acp::{AcpSession, AcpAgentConfig, PermissionPolicy};
//! use std::sync::Arc;
//!
//! let config = AcpAgentConfig::new("kiro-cli acp --trust-all-tools")
//!     .working_dir("/path/to/project");
//!
//! let mut session = AcpSession::start(config, Arc::new(PermissionPolicy::AutoApprove)).await?;
//!
//! // First prompt — Kiro reads the project structure
//! let r1 = session.prompt("List the files in src/").await?;
//! println!("{}", r1.text);
//!
//! // Second prompt — Kiro already has context from the first
//! let r2 = session.prompt("Now explain what main.rs does").await?;
//! println!("{}", r2.text);
//!
//! // Clean shutdown
//! session.close().await?;
//! ```

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::schema::{
    InitializeRequest, ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use agent_client_protocol_tokio::AcpAgent;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::connection::AcpAgentConfig;
use crate::error::{AcpError, Result};
use crate::permissions::{
    PermissionDecision, PermissionOption, PermissionPolicy, PermissionRequest,
};

/// Result of a prompt sent to a persistent session.
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// The text response from the agent.
    pub text: String,
    /// Wall-clock duration of this prompt.
    pub duration: Duration,
    /// Number of prompts sent in this session so far (including this one).
    pub prompt_count: u32,
}

/// A persistent connection to an ACP agent with session reuse.
///
/// The agent process stays alive between prompts, preserving conversation
/// context and reducing spawn overhead. Use this when you need multiple
/// interactions with the same agent in sequence.
pub struct AcpSession {
    config: AcpAgentConfig,
    #[allow(dead_code)]
    policy: Arc<PermissionPolicy>,
    prompt_count: u32,
    started_at: Instant,
    /// Inner state — None after close()
    inner: Option<SessionInner>,
}

/// Holds the actual connection state.
/// We use a channel-based approach: the ACP connection runs in a background task,
/// and we send prompts to it via channels.
struct SessionInner {
    prompt_tx: tokio::sync::mpsc::Sender<SessionCommand>,
    result_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<SessionResult>>>,
}

enum SessionCommand {
    Prompt(String),
    Cancel,
    Close,
}

enum SessionResult {
    Response(String),
    Error(String),
    Cancelled,
    Closed,
}

impl AcpSession {
    /// Start a new persistent session with an ACP agent.
    ///
    /// Spawns the agent process, performs the ACP handshake, and creates a session.
    /// The connection stays alive until [`close()`](Self::close) is called or the
    /// session is dropped.
    pub async fn start(config: AcpAgentConfig, policy: Arc<PermissionPolicy>) -> Result<Self> {
        info!(command = %config.command, cwd = %config.working_dir.display(), "starting persistent ACP session");

        let command_with_env =
            crate::connection::build_command_with_env(&config.command, &config.env);

        let agent = AcpAgent::from_str(&command_with_env).map_err(|e| {
            AcpError::InvalidConfig(format!("invalid command '{}': {e}", config.command))
        })?;

        let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::channel::<SessionCommand>(1);
        let (result_tx, result_rx) = tokio::sync::mpsc::channel::<SessionResult>(1);

        let working_dir = config.working_dir.clone();
        let policy_clone = policy.clone();

        // Spawn the ACP connection in a background task
        tokio::spawn(async move {
            let result_tx_err = result_tx.clone();
            let outcome = Client
                .builder()
                .on_receive_request(
                    {
                        let policy = policy_clone.clone();
                        async move |request: RequestPermissionRequest,
                                    responder,
                                    _cx: ConnectionTo<Agent>| {
                            let title = request
                                .options
                                .first()
                                .map(|o| o.name.to_string())
                                .unwrap_or_else(|| "Unknown operation".to_string());

                            let perm_request = PermissionRequest {
                                title: title.clone(),
                                options: request
                                    .options
                                    .iter()
                                    .map(|o| PermissionOption {
                                        id: o.option_id.to_string(),
                                        name: o.name.to_string(),
                                    })
                                    .collect(),
                            };

                            let decision = policy.decide(&perm_request);
                            match &decision {
                                PermissionDecision::Allow(option_id) => {
                                    debug!(title = %title, "ACP permission granted");
                                    responder.respond(RequestPermissionResponse::new(
                                        RequestPermissionOutcome::Selected(
                                            SelectedPermissionOutcome::new(option_id.clone()),
                                        ),
                                    ))
                                }
                                PermissionDecision::Deny => {
                                    warn!(title = %title, "ACP permission DENIED");
                                    responder.respond(RequestPermissionResponse::new(
                                        RequestPermissionOutcome::Cancelled,
                                    ))
                                }
                            }
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
                    // Initialize
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;

                    // Create session and enter the prompt loop
                    connection
                        .build_session(&working_dir)
                        .block_task()
                        .run_until(async |mut session| {
                            // Process commands from the main task
                            while let Some(cmd) = prompt_rx.recv().await {
                                match cmd {
                                    SessionCommand::Prompt(text) => {
                                        match session.send_prompt(&text) {
                                            Ok(()) => match session.read_to_string().await {
                                                Ok(response) => {
                                                    let _ = result_tx
                                                        .send(SessionResult::Response(response))
                                                        .await;
                                                }
                                                Err(e) => {
                                                    let _ = result_tx
                                                        .send(SessionResult::Error(e.to_string()))
                                                        .await;
                                                }
                                            },
                                            Err(e) => {
                                                let _ = result_tx
                                                    .send(SessionResult::Error(e.to_string()))
                                                    .await;
                                            }
                                        }
                                    }
                                    SessionCommand::Cancel => {
                                        let _ = result_tx.send(SessionResult::Cancelled).await;
                                        break;
                                    }
                                    SessionCommand::Close => {
                                        let _ = result_tx.send(SessionResult::Closed).await;
                                        break;
                                    }
                                }
                            }
                            Ok(())
                        })
                        .await?;

                    Ok(())
                })
                .await;

            if let Err(e) = outcome {
                warn!(error = %e, "ACP session background task ended with error");
                let _ = result_tx_err.send(SessionResult::Error(e.to_string())).await;
            }
        });

        Ok(Self {
            config,
            policy,
            prompt_count: 0,
            started_at: Instant::now(),
            inner: Some(SessionInner { prompt_tx, result_rx: Arc::new(Mutex::new(result_rx)) }),
        })
    }

    /// Send a prompt to the agent within the existing session.
    ///
    /// The agent retains context from previous prompts in this session,
    /// so you don't need to re-explain project structure or repeat instructions.
    pub async fn prompt(&mut self, text: &str) -> Result<PromptResult> {
        let inner = self
            .inner
            .as_ref()
            .ok_or_else(|| AcpError::ConnectionLost("session already closed".into()))?;

        let start = Instant::now();
        self.prompt_count += 1;

        debug!(
            prompt_count = self.prompt_count,
            prompt_len = text.len(),
            "sending prompt to persistent session"
        );

        inner
            .prompt_tx
            .send(SessionCommand::Prompt(text.to_string()))
            .await
            .map_err(|_| AcpError::ConnectionLost("agent process exited".into()))?;

        let mut rx = inner.result_rx.lock().await;
        match rx.recv().await {
            Some(SessionResult::Response(text)) => Ok(PromptResult {
                text,
                duration: start.elapsed(),
                prompt_count: self.prompt_count,
            }),
            Some(SessionResult::Error(e)) => Err(AcpError::Protocol(e)),
            Some(SessionResult::Cancelled) => {
                Err(AcpError::ConnectionLost("prompt cancelled".into()))
            }
            Some(SessionResult::Closed) => Err(AcpError::ConnectionLost("session closed".into())),
            None => Err(AcpError::ConnectionLost("agent process exited".into())),
        }
    }

    /// Close the session and terminate the agent process.
    pub async fn close(&mut self) -> Result<()> {
        if let Some(inner) = self.inner.take() {
            let _ = inner.prompt_tx.send(SessionCommand::Close).await;
            info!(
                prompt_count = self.prompt_count,
                uptime = ?self.started_at.elapsed(),
                "ACP session closed"
            );
        }
        Ok(())
    }

    /// Cancel the currently running prompt.
    ///
    /// If the agent is processing a prompt, this terminates the session and
    /// restarts it. The next call to [`prompt()`](Self::prompt) will work
    /// on a fresh session context.
    pub async fn cancel(&mut self) -> Result<()> {
        if let Some(inner) = &self.inner {
            info!("cancelling in-progress ACP prompt");
            let _ = inner.prompt_tx.send(SessionCommand::Cancel).await;
            // Drain the result channel
            let mut rx = inner.result_rx.lock().await;
            let _ = rx.recv().await;
        }
        // Close and restart
        self.inner = None;
        let mut new_session = AcpSession::start(self.config.clone(), self.policy.clone()).await?;
        self.inner = new_session.inner.take();
        info!("ACP session restarted after cancel");
        Ok(())
    }

    /// Number of prompts sent in this session.
    pub fn prompt_count(&self) -> u32 {
        self.prompt_count
    }

    /// How long this session has been alive.
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Whether the session is still connected.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    /// Get the working directory for this session.
    pub fn working_dir(&self) -> &PathBuf {
        &self.config.working_dir
    }
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        if self.inner.is_some() {
            warn!("AcpSession dropped without explicit close — agent process may linger");
        }
    }
}
