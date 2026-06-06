//! Pagination types — cursor-based pagination.

use serde::{Deserialize, Serialize};

/// Cursor-paginated list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub data: Vec<T>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Parameters for list endpoints.
#[derive(Debug, Clone, Default)]
pub struct ListParams {
    /// Maximum number of items to return.
    pub limit: Option<u32>,
    /// Cursor for pagination (from a previous response's `next_cursor`).
    pub cursor: Option<String>,
}
