//! Session lifecycle methods for [`EnterpriseClient`].
//!
//! Implements Requirements 4.1–4.9:
//! - `create_session` — convenience method (agent_id + optional env_id)
//! - `create_session_full` — POST /sessions (with Idempotency-Key header)
//! - `get_session` — GET /sessions/{id}
//! - `list_sessions` — GET /sessions (cursor pagination via limit/cursor query params)
//! - `pause_session` — POST /sessions/{id}/pause
//! - `resume_session` — POST /sessions/{id}/resume
//! - `archive_session` — POST /sessions/{id}/archive
//! - `delete_session` — DELETE /sessions/{id}

use crate::Result;
use crate::client::EnterpriseClient;
use crate::idempotency::IDEMPOTENCY_KEY_HEADER;
use crate::response::{handle_empty_response, handle_response};
use crate::retry::{RetryPolicy, execute_create_with_retry, execute_with_retry};
use crate::types::pagination::{ListParams, ListResponse};
use crate::types::session::{CreateSessionParams, Session};

impl EnterpriseClient {
    /// Create a new session with minimal parameters (convenience method).
    ///
    /// Builds a `CreateSessionParams` from the provided `agent_id` and optional
    /// `env_id`, then delegates to [`create_session_full`](Self::create_session_full).
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The ID of the agent to run in this session (e.g., `"agt_abc123"`)
    /// * `env_id` - Optional environment ID for execution context
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let session = client.create_session("agt_abc123", None).await?;
    /// println!("Session: {} (status: {:?})", session.id, session.status);
    ///
    /// // With an environment
    /// let session = client.create_session("agt_abc123", Some("env_xyz")).await?;
    /// ```
    pub async fn create_session(&self, agent_id: &str, env_id: Option<&str>) -> Result<Session> {
        let params = CreateSessionParams {
            agent_id: agent_id.to_string(),
            environment_id: env_id.map(|s| s.to_string()),
            ..Default::default()
        };
        self.create_session_full(params).await
    }

    /// Create a new session with full parameter control.
    ///
    /// POSTs to `/sessions` with an `Idempotency-Key` header for retry safety.
    /// Allows setting title, vault IDs, and metadata in addition to the required
    /// agent ID.
    ///
    /// # Arguments
    ///
    /// * `params` - Full session creation parameters
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::CreateSessionParams;
    ///
    /// let session = client.create_session_full(CreateSessionParams {
    ///     agent_id: "agt_abc123".into(),
    ///     title: Some("Debug session".into()),
    ///     vault_ids: vec!["vault_xyz".into()],
    ///     ..Default::default()
    /// }).await?;
    /// ```
    pub async fn create_session_full(&self, params: CreateSessionParams) -> Result<Session> {
        let url = self.build_url("/sessions");
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let body = serde_json::to_vec(&params)?;

        let response = execute_create_with_retry(&policy, |idempotency_key| {
            let url = url.clone();
            let headers = headers.clone();
            let body = body.clone();
            async move {
                reqwest::Client::new()
                    .post(&url)
                    .headers(headers)
                    .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
                    .body(body)
                    .send()
                    .await
            }
        })
        .await?;

        handle_response(response).await
    }

    /// Get a session by ID.
    ///
    /// GETs `/sessions/{id}` and returns the full session object including
    /// current status and usage.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier (e.g., `"ses_abc123"`)
    ///
    /// # Errors
    ///
    /// Returns `EnterpriseError::NotFound` if the session does not exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let session = client.get_session("ses_abc123").await?;
    /// println!("Status: {:?}, Usage: {:?}", session.status, session.usage);
    /// ```
    pub async fn get_session(&self, session_id: &str) -> Result<Session> {
        let url = self.build_url(&format!("/sessions/{session_id}"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            async move { reqwest::Client::new().get(&url).headers(headers).send().await }
        })
        .await?;

        handle_response(response).await
    }

    /// List sessions with optional cursor pagination.
    ///
    /// GETs `/sessions` with optional `limit` and `cursor` query parameters.
    /// Returns a paginated `ListResponse<Session>`.
    ///
    /// # Arguments
    ///
    /// * `params` - Optional pagination parameters (limit, cursor)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::ListParams;
    ///
    /// // First page
    /// let page = client.list_sessions(Some(ListParams {
    ///     limit: Some(10),
    ///     cursor: None,
    /// })).await?;
    ///
    /// for session in &page.data {
    ///     println!("  {} ({:?})", session.id, session.status);
    /// }
    ///
    /// // Next page
    /// if page.has_more {
    ///     let next = client.list_sessions(Some(ListParams {
    ///         limit: Some(10),
    ///         cursor: page.next_cursor,
    ///     })).await?;
    /// }
    /// ```
    pub async fn list_sessions(&self, params: Option<ListParams>) -> Result<ListResponse<Session>> {
        let url = self.build_url("/sessions");
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let query_params = build_session_list_query(params.as_ref());

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            let query_params = query_params.clone();
            async move {
                reqwest::Client::new().get(&url).headers(headers).query(&query_params).send().await
            }
        })
        .await?;

        handle_response(response).await
    }

    /// Pause a running session.
    ///
    /// POSTs to `/sessions/{id}/pause`. Transitions the session from `Running`
    /// or `Idle` to `Paused` state. The session can later be resumed with
    /// [`resume_session`](Self::resume_session).
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to pause
    ///
    /// # Errors
    ///
    /// Returns `EnterpriseError::Conflict` if the session is not in a pausable state.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let session = client.pause_session("ses_abc123").await?;
    /// assert_eq!(session.status, SessionStatus::Paused);
    /// ```
    pub async fn pause_session(&self, session_id: &str) -> Result<Session> {
        let url = self.build_url(&format!("/sessions/{session_id}/pause"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            async move { reqwest::Client::new().post(&url).headers(headers).send().await }
        })
        .await?;

        handle_response(response).await
    }

    /// Resume a paused session.
    ///
    /// POSTs to `/sessions/{id}/resume`. Transitions the session from `Paused`
    /// back to `Running` state.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to resume
    ///
    /// # Errors
    ///
    /// Returns `EnterpriseError::Conflict` if the session is not paused.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let session = client.resume_session("ses_abc123").await?;
    /// assert_eq!(session.status, SessionStatus::Running);
    /// ```
    pub async fn resume_session(&self, session_id: &str) -> Result<Session> {
        let url = self.build_url(&format!("/sessions/{session_id}/resume"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            async move { reqwest::Client::new().post(&url).headers(headers).send().await }
        })
        .await?;

        handle_response(response).await
    }

    /// Archive a session.
    ///
    /// POSTs to `/sessions/{id}/archive`. Moves the session to `Archived` state.
    /// Archived sessions are read-only and no longer accept events or state transitions.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to archive
    ///
    /// # Errors
    ///
    /// Returns `EnterpriseError::Conflict` if the session is already archived or deleted.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let session = client.archive_session("ses_abc123").await?;
    /// assert_eq!(session.status, SessionStatus::Archived);
    /// ```
    pub async fn archive_session(&self, session_id: &str) -> Result<Session> {
        let url = self.build_url(&format!("/sessions/{session_id}/archive"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            async move { reqwest::Client::new().post(&url).headers(headers).send().await }
        })
        .await?;

        handle_response(response).await
    }

    /// Delete a session permanently.
    ///
    /// DELETEs `/sessions/{id}`. This is irreversible — the session and all its
    /// events are permanently deleted.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to delete
    ///
    /// # Errors
    ///
    /// Returns `EnterpriseError::NotFound` if the session does not exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.delete_session("ses_abc123").await?;
    /// ```
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let url = self.build_url(&format!("/sessions/{session_id}"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            async move { reqwest::Client::new().delete(&url).headers(headers).send().await }
        })
        .await?;

        handle_empty_response(response).await
    }
}

/// Build query parameters for session list endpoints from optional `ListParams`.
fn build_session_list_query(params: Option<&ListParams>) -> Vec<(String, String)> {
    let mut query = Vec::new();
    if let Some(p) = params {
        if let Some(limit) = p.limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(ref cursor) = p.cursor {
            query.push(("cursor".to_string(), cursor.clone()));
        }
    }
    query
}

#[cfg(test)]
mod tests {
    use crate::EnterpriseClient;
    use crate::types::pagination::ListResponse;
    use crate::types::session::{CreateSessionParams, Session, SessionStatus};

    #[test]
    fn test_create_session_params_serialization() {
        let params = CreateSessionParams {
            agent_id: "agt_123".into(),
            environment_id: Some("env_456".into()),
            title: Some("Test Session".into()),
            vault_ids: vec!["vault_789".into()],
            metadata: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["agent_id"], "agt_123");
        assert_eq!(json["environment_id"], "env_456");
        assert_eq!(json["title"], "Test Session");
        assert_eq!(json["vault_ids"], serde_json::json!(["vault_789"]));
        // metadata is None + skip_serializing_if, so absent
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_create_session_params_minimal_serialization() {
        let params = CreateSessionParams { agent_id: "agt_123".into(), ..Default::default() };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["agent_id"], "agt_123");
        // Optional/empty fields should not appear
        assert!(json.get("environment_id").is_none());
        assert!(json.get("title").is_none());
        assert!(json.get("vault_ids").is_none());
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_session_deserialization() {
        let json = serde_json::json!({
            "id": "ses_abc123",
            "agent_id": "agt_xyz",
            "environment_id": "env_456",
            "status": "idle",
            "title": "My Session",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "total_tokens": 150,
                "cost_usd": 0.002
            },
            "created_at": "2026-06-01T00:00:00Z",
            "updated_at": "2026-06-01T01:00:00Z"
        });
        let session: Session = serde_json::from_value(json).unwrap();
        assert_eq!(session.id, "ses_abc123");
        assert_eq!(session.agent_id, "agt_xyz");
        assert_eq!(session.environment_id, Some("env_456".into()));
        assert_eq!(session.status, SessionStatus::Idle);
        assert_eq!(session.title, Some("My Session".into()));
        let usage = session.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cost_usd, Some(0.002));
    }

    #[test]
    fn test_session_deserialization_minimal() {
        let json = serde_json::json!({
            "id": "ses_001",
            "agent_id": "agt_001",
            "status": "queued",
            "created_at": "2026-06-01T00:00:00Z",
            "updated_at": "2026-06-01T00:00:00Z"
        });
        let session: Session = serde_json::from_value(json).unwrap();
        assert_eq!(session.id, "ses_001");
        assert_eq!(session.agent_id, "agt_001");
        assert_eq!(session.environment_id, None);
        assert_eq!(session.status, SessionStatus::Queued);
        assert_eq!(session.title, None);
        assert!(session.usage.is_none());
    }

    #[test]
    fn test_session_status_all_variants() {
        let variants = [
            ("queued", SessionStatus::Queued),
            ("running", SessionStatus::Running),
            ("idle", SessionStatus::Idle),
            ("paused", SessionStatus::Paused),
            ("completed", SessionStatus::Completed),
            ("failed", SessionStatus::Failed),
            ("archived", SessionStatus::Archived),
        ];

        for (json_str, expected) in variants {
            let json = format!("\"{json_str}\"");
            let status: SessionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, expected, "Failed for variant: {json_str}");
        }
    }

    #[test]
    fn test_list_response_deserialization() {
        let json = serde_json::json!({
            "data": [
                {
                    "id": "ses_001",
                    "agent_id": "agt_001",
                    "status": "idle",
                    "created_at": "2026-06-01T00:00:00Z",
                    "updated_at": "2026-06-01T00:00:00Z"
                },
                {
                    "id": "ses_002",
                    "agent_id": "agt_001",
                    "status": "paused",
                    "created_at": "2026-06-01T00:00:00Z",
                    "updated_at": "2026-06-01T00:00:00Z"
                }
            ],
            "next_cursor": "cursor_abc",
            "has_more": true
        });
        let list: ListResponse<Session> = serde_json::from_value(json).unwrap();
        assert_eq!(list.data.len(), 2);
        assert_eq!(list.data[0].id, "ses_001");
        assert_eq!(list.data[1].id, "ses_002");
        assert_eq!(list.next_cursor, Some("cursor_abc".into()));
        assert!(list.has_more);
    }

    #[test]
    fn test_list_response_empty() {
        let json = serde_json::json!({
            "data": [],
            "has_more": false
        });
        let list: ListResponse<Session> = serde_json::from_value(json).unwrap();
        assert!(list.data.is_empty());
        assert_eq!(list.next_cursor, None);
        assert!(!list.has_more);
    }

    #[test]
    fn test_build_url_for_sessions() {
        let client = EnterpriseClient::new("key").unwrap();
        let url = client.build_url("/sessions");
        assert!(url.ends_with("/sessions"));

        let url = client.build_url("/sessions/ses_123");
        assert!(url.ends_with("/sessions/ses_123"));

        let url = client.build_url("/sessions/ses_123/pause");
        assert!(url.ends_with("/sessions/ses_123/pause"));

        let url = client.build_url("/sessions/ses_123/resume");
        assert!(url.ends_with("/sessions/ses_123/resume"));

        let url = client.build_url("/sessions/ses_123/archive");
        assert!(url.ends_with("/sessions/ses_123/archive"));
    }

    #[test]
    fn test_build_session_list_query_empty() {
        let query = super::build_session_list_query(None);
        assert!(query.is_empty());
    }

    #[test]
    fn test_build_session_list_query_limit_only() {
        use crate::types::pagination::ListParams;
        let params = ListParams { limit: Some(20), cursor: None };
        let query = super::build_session_list_query(Some(&params));
        assert_eq!(query, vec![("limit".to_string(), "20".to_string())]);
    }

    #[test]
    fn test_build_session_list_query_cursor_only() {
        use crate::types::pagination::ListParams;
        let params = ListParams { limit: None, cursor: Some("abc123".into()) };
        let query = super::build_session_list_query(Some(&params));
        assert_eq!(query, vec![("cursor".to_string(), "abc123".to_string())]);
    }

    #[test]
    fn test_build_session_list_query_both() {
        use crate::types::pagination::ListParams;
        let params = ListParams { limit: Some(5), cursor: Some("next_page".into()) };
        let query = super::build_session_list_query(Some(&params));
        assert_eq!(
            query,
            vec![
                ("limit".to_string(), "5".to_string()),
                ("cursor".to_string(), "next_page".to_string()),
            ]
        );
    }
}
