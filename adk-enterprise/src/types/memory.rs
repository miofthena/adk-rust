//! Memory types (beta) — persistent cross-session memory.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A memory store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStore {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Parameters for creating a memory store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemoryStoreParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A memory entry within a store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub store_id: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Parameters for creating a memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemoryParams {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Parameters for updating a memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMemoryParams {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// A memory version entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryVersion {
    pub version: u64,
    pub content: String,
    pub created_at: String,
}
