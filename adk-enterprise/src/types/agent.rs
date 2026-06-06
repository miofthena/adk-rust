//! Agent types — managed agent configuration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::model_ref::ModelRef;
use super::tools::{McpServerConfig, PermissionPolicy, SkillRef, ToolConfig};

/// A managed agent configuration as returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub model: ModelRef,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub archived_at: Option<String>,
}

/// Parameters for creating a new agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateAgentParams {
    pub name: String,
    pub model: ModelRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Parameters for updating an existing agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateAgentParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}
