//! Session types — stateful agent execution context.

use serde::{Deserialize, Serialize};

/// A session as returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent_id: String,
    #[serde(default)]
    pub environment_id: Option<String>,
    pub status: SessionStatus,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    pub created_at: String,
    pub updated_at: String,
}

/// Session lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Queued,
    Running,
    Idle,
    Paused,
    Completed,
    Failed,
    Archived,
}

/// Token usage and cost for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

/// Parameters for creating a session with full control.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateSessionParams {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vault_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
}
