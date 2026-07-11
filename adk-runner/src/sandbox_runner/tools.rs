//! Sandbox tool implementations for shell and filesystem capabilities.
//!
//! This module provides the tool structs that are bound to the agent when
//! sandbox capabilities are enabled:
//!
//! - [`ExecCommandTool`] — executes shell commands in the sandbox workspace
//! - [`ReadFileTool`] — reads files from the sandbox workspace
//! - [`WriteFileTool`] — writes files to the sandbox workspace
//! - [`ListDirTool`] — lists directory entries in the sandbox workspace
//! - [`ApplyPatchTool`] — applies unified diff patches to the sandbox workspace
//!
//! All tools return errors as JSON in the response (not as Rust `Result::Err`)
//! so the LLM can reason about failures and self-correct.

use adk_core::Tool;
use adk_sandbox::workspace::{ExecOptions, SandboxSession};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Fallback per-command timeout when none is configured (120s).
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(120);
/// Default cap on captured output per stream (1 MiB).
const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Default cap on the stdin payload (1 MiB).
const DEFAULT_MAX_STDIN_BYTES: usize = 1024 * 1024;
/// Default grace between SIGTERM and SIGKILL when terminating the group (1s).
const DEFAULT_TERMINATION_GRACE: Duration = Duration::from_secs(1);

/// Timeouts and byte caps enforced by [`ExecCommandTool`].
///
/// Fields are public so an outer runtime can drive them from config. Use
/// [`ExecLimits::from_timeout`] for the backward-compatible defaults.
#[derive(Debug, Clone)]
pub struct ExecLimits {
    /// Timeout used when the caller does not request one via `timeout_secs`.
    pub default_timeout: Duration,
    /// Upper bound a caller-requested `timeout_secs` is clamped to.
    pub max_timeout: Duration,
    /// Cap on captured stdout/stderr bytes (each stream).
    pub max_output_bytes: usize,
    /// Reject `stdin` arguments larger than this many bytes.
    pub max_stdin_bytes: usize,
    /// Grace between the SIGTERM and SIGKILL sent to the process group.
    pub termination_grace: Duration,
}

impl ExecLimits {
    /// Build limits from a single command timeout, using sane caps for the
    /// rest. `default_timeout == max_timeout == timeout`, so the
    /// backward-compatible [`ExecCommandTool::new`] behaves as before — a
    /// caller-supplied `timeout_secs` can only lower the effective timeout.
    pub fn from_timeout(timeout: Duration) -> Self {
        Self {
            default_timeout: timeout,
            max_timeout: timeout,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_stdin_bytes: DEFAULT_MAX_STDIN_BYTES,
            termination_grace: DEFAULT_TERMINATION_GRACE,
        }
    }
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self::from_timeout(DEFAULT_EXEC_TIMEOUT)
    }
}

/// Tool that executes shell commands in the sandbox workspace.
///
/// Bound to the agent when [`Capability::Shell`](adk_sandbox::workspace::Capability::Shell)
/// is enabled. Returns JSON with `stdout`, `stderr`, `exit_code`, `timed_out`,
/// `cancelled`, and `truncated` fields. The tool name and enforced limits are
/// configurable (see [`ExecCommandTool::with_name`] / [`ExecCommandTool::with_limits`])
/// so an outer runtime can expose it as a single unified `exec` tool.
pub struct ExecCommandTool {
    /// The live sandbox session to execute commands against.
    pub(crate) session: Arc<dyn SandboxSession>,
    /// Tool name exposed to the model (default `"exec_command"`).
    pub(crate) name: String,
    /// Timeouts and byte caps enforced on each invocation.
    pub(crate) limits: ExecLimits,
}

impl ExecCommandTool {
    /// Creates a new `ExecCommandTool` bound to the given session, named
    /// `exec_command`, with limits derived from `timeout`.
    pub fn new(session: Arc<dyn SandboxSession>, timeout: Duration) -> Self {
        Self {
            session,
            name: "exec_command".to_string(),
            limits: ExecLimits::from_timeout(timeout),
        }
    }

    /// Override the tool name exposed to the model (e.g. `"exec"`).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Override the enforced limits (timeouts and byte caps).
    pub fn with_limits(mut self, limits: ExecLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Execute a shell command in the sandbox workspace. Supports an optional \
         working directory, stdin, and a per-command timeout; captured output is \
         byte-capped and long-running commands are cancelled with their whole \
         process group"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory relative to workspace root"
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Optional per-command timeout in seconds (clamped to the configured maximum)"
                },
                "stdin": {
                    "type": "string",
                    "description": "Optional data written to the command's standard input"
                }
            },
            "required": ["command"]
        }))
    }

    async fn execute(
        &self,
        ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd.to_string(),
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'command'"
                }));
            }
        };

        let working_dir = args.get("working_dir").and_then(|v| v.as_str()).map(str::to_string);

        // Clamp a caller-requested timeout to the configured ceiling; fall back
        // to the default when absent or non-positive.
        let timeout = match args.get("timeout_secs").and_then(|v| v.as_f64()) {
            Some(secs) if secs > 0.0 => Duration::from_secs_f64(secs).min(self.limits.max_timeout),
            _ => self.limits.default_timeout,
        };

        // Enforce the stdin cap up front; reject over-cap structurally (never panic).
        let stdin = match args.get("stdin").and_then(|v| v.as_str()) {
            Some(s) if s.len() > self.limits.max_stdin_bytes => {
                return Ok(serde_json::json!({
                    "error": "stdin_too_large",
                    "message": format!(
                        "stdin is {} bytes, exceeds the {}-byte limit",
                        s.len(),
                        self.limits.max_stdin_bytes
                    )
                }));
            }
            Some(s) => Some(s.as_bytes().to_vec()),
            None => None,
        };

        let opts = ExecOptions {
            working_dir,
            stdin,
            max_output_bytes: Some(self.limits.max_output_bytes),
            timeout: Some(timeout),
            termination_grace: self.limits.termination_grace,
            // Session-scoped cancellation token, when the runner threaded one through.
            cancel: ctx.cancellation_token(),
        };

        match self.session.exec_command_opts(&command, opts).await {
            Ok(output) => Ok(serde_json::json!({
                "stdout": output.stdout,
                "stderr": output.stderr,
                "exit_code": output.exit_code,
                "timed_out": output.timed_out,
                "cancelled": output.cancelled,
                "truncated": output.truncated
            })),
            Err(e) => {
                // Return error as JSON for LLM self-correction
                let error_type = match &e {
                    adk_sandbox::SandboxError::PathTraversal { .. } => "path_traversal",
                    _ => "execution_error",
                };
                Ok(serde_json::json!({
                    "error": error_type,
                    "message": e.to_string()
                }))
            }
        }
    }
}

/// Tool that reads files from the sandbox workspace.
///
/// Bound to the agent when [`Capability::Filesystem`](adk_sandbox::workspace::Capability::Filesystem)
/// is enabled.
pub struct ReadFileTool {
    /// The live sandbox session to read files from.
    pub(crate) session: Arc<dyn SandboxSession>,
}

impl ReadFileTool {
    /// Creates a new `ReadFileTool` bound to the given session.
    pub fn new(session: Arc<dyn SandboxSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a file from the sandbox workspace"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to workspace root"
                }
            },
            "required": ["path"]
        }))
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'path'"
                }));
            }
        };

        match self.session.read_file(path).await {
            Ok(content) => {
                // Return file contents as a string
                let text = String::from_utf8_lossy(&content);
                Ok(serde_json::json!({
                    "content": text
                }))
            }
            Err(e) => {
                let error_type = match &e {
                    adk_sandbox::SandboxError::PathTraversal { .. } => "path_traversal",
                    adk_sandbox::SandboxError::ExecutionFailed(msg)
                        if msg.contains("not found")
                            || msg.contains("No such file")
                            || msg.contains("does not exist") =>
                    {
                        "not_found"
                    }
                    _ => "read_error",
                };
                Ok(serde_json::json!({
                    "error": error_type,
                    "message": e.to_string(),
                    "path": path
                }))
            }
        }
    }
}

/// Tool that writes files to the sandbox workspace.
///
/// Bound to the agent when [`Capability::Filesystem`](adk_sandbox::workspace::Capability::Filesystem)
/// is enabled.
pub struct WriteFileTool {
    /// The live sandbox session to write files to.
    pub(crate) session: Arc<dyn SandboxSession>,
}

impl WriteFileTool {
    /// Creates a new `WriteFileTool` bound to the given session.
    pub fn new(session: Arc<dyn SandboxSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file in the sandbox workspace"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to workspace root"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        }))
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'path'"
                }));
            }
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'content'"
                }));
            }
        };

        match self.session.write_file(path, content.as_bytes()).await {
            Ok(()) => Ok(serde_json::json!({
                "success": true,
                "path": path,
                "message": format!("Successfully wrote {} bytes to '{path}'", content.len())
            })),
            Err(e) => {
                let error_type = match &e {
                    adk_sandbox::SandboxError::PathTraversal { .. } => "path_traversal",
                    _ => "write_error",
                };
                Ok(serde_json::json!({
                    "error": error_type,
                    "message": e.to_string(),
                    "path": path
                }))
            }
        }
    }
}

/// Tool that lists directory entries in the sandbox workspace.
///
/// Bound to the agent when [`Capability::Filesystem`](adk_sandbox::workspace::Capability::Filesystem)
/// is enabled.
pub struct ListDirTool {
    /// The live sandbox session to list directories from.
    pub(crate) session: Arc<dyn SandboxSession>,
}

impl ListDirTool {
    /// Creates a new `ListDirTool` bound to the given session.
    pub fn new(session: Arc<dyn SandboxSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory within the sandbox workspace"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path relative to workspace root"
                }
            },
            "required": ["path"]
        }))
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'path'"
                }));
            }
        };

        match self.session.list_dir(path).await {
            Ok(entries) => {
                // Serialize entries as JSON array of {name, type}
                let entries_json: Vec<Value> = entries
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "name": entry.name,
                            "type": serde_json::to_value(entry.entry_type)
                                .unwrap_or(Value::String("unknown".to_string()))
                        })
                    })
                    .collect();
                Ok(Value::Array(entries_json))
            }
            Err(e) => {
                let error_type = match &e {
                    adk_sandbox::SandboxError::PathTraversal { .. } => "path_traversal",
                    _ => "list_error",
                };
                Ok(serde_json::json!({
                    "error": error_type,
                    "message": e.to_string(),
                    "path": path
                }))
            }
        }
    }
}

/// Tool that applies unified diff patches to the sandbox workspace.
///
/// Bound to the agent when [`Capability::Filesystem`](adk_sandbox::workspace::Capability::Filesystem)
/// is enabled.
pub struct ApplyPatchTool {
    /// The live sandbox session to apply patches to.
    pub(crate) session: Arc<dyn SandboxSession>,
}

impl ApplyPatchTool {
    /// Creates a new `ApplyPatchTool` bound to the given session.
    pub fn new(session: Arc<dyn SandboxSession>) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to the sandbox workspace"
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content to apply"
                }
            },
            "required": ["patch"]
        }))
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let patch = match args.get("patch").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(serde_json::json!({
                    "error": "invalid_arguments",
                    "message": "Missing required parameter 'patch'"
                }));
            }
        };

        match self.session.apply_patch(patch).await {
            Ok(()) => Ok(serde_json::json!({
                "success": true,
                "message": "Patch applied successfully"
            })),
            Err(e) => {
                let error_type = match &e {
                    adk_sandbox::SandboxError::PathTraversal { .. } => "path_traversal",
                    _ => "patch_error",
                };
                Ok(serde_json::json!({
                    "error": error_type,
                    "message": e.to_string()
                }))
            }
        }
    }
}
