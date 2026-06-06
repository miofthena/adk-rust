//! Environment types — execution sandbox configuration.

use serde::{Deserialize, Serialize};

/// An execution environment (sandbox) as returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub environment_type: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub archived_at: Option<String>,
}

/// Parameters for creating a new environment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateEnvironmentParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

impl CreateEnvironmentParams {
    /// Create params for a cloud environment with unrestricted networking.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let params = CreateEnvironmentParams::cloud("my-env");
    /// ```
    pub fn cloud(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            environment_type: Some("cloud".to_string()),
            config: Some(serde_json::json!({
                "type": "cloud",
                "networking": {"type": "unrestricted"}
            })),
        }
    }

    /// Create params for a self-hosted environment.
    ///
    /// Self-hosted environments run tool execution on your own infrastructure.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let params = CreateEnvironmentParams::self_hosted("my-env");
    /// ```
    pub fn self_hosted(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            environment_type: Some("self_hosted".to_string()),
            config: Some(serde_json::json!({"type": "self_hosted"})),
        }
    }

    /// Set the description for this environment.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}
