//! Vault and Credential types (beta).

use serde::{Deserialize, Serialize};

/// A vault container for MCP credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub archived_at: Option<String>,
}

/// Parameters for creating a vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVaultParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A stored credential within a vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub name: String,
    pub url: String,
    pub credential_type: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Parameters for creating a credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCredentialParams {
    pub name: String,
    pub url: String,
    pub credential_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

impl CreateCredentialParams {
    /// Create a static bearer credential.
    pub fn static_bearer(
        name: impl Into<String>,
        url: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            credential_type: "static_bearer".into(),
            token: Some(token.into()),
            access_token: None,
            expires_at: None,
            refresh_token: None,
        }
    }

    /// Create an MCP OAuth credential.
    pub fn mcp_oauth(
        name: impl Into<String>,
        url: impl Into<String>,
        access_token: impl Into<String>,
        expires_at: impl Into<String>,
        refresh_token: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            credential_type: "mcp_oauth".into(),
            token: None,
            access_token: Some(access_token.into()),
            expires_at: Some(expires_at.into()),
            refresh_token,
        }
    }
}

/// Parameters for updating a credential.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateCredentialParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

/// Result of credential validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialValidation {
    pub valid: bool,
    #[serde(default)]
    pub message: Option<String>,
}
