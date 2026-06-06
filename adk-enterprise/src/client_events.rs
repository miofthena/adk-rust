//! Event dispatch and history methods for [`EnterpriseClient`].
//!
//! Implements Requirements 5.1–5.4 (event dispatch):
//! - `send_event` — POST /sessions/{id}/events (with retry)
//! - `send_message` — convenience for `user.message`
//! - `interrupt` — convenience for `user.interrupt`
//! - `allow_tool` — convenience for `user.tool_confirmation` (allow)
//! - `deny_tool` — convenience for `user.tool_confirmation` (deny)
//! - `custom_tool_result` — convenience for `user.custom_tool_result`
//! - `define_outcome` — convenience for `user.define_outcome`
//!
//! Implements Requirements 7.1, 7.2 (event history):
//! - `list_events` — GET /sessions/{id}/events (cursor pagination)

use crate::Result;
use crate::client::EnterpriseClient;
use crate::response::{handle_empty_response, handle_response};
use crate::retry::{RetryPolicy, execute_with_retry};
use crate::types::events::{StoredEvent, UserEvent};
use crate::types::pagination::{ListParams, ListResponse};

impl EnterpriseClient {
    // ─── Event Dispatch (Requirements 5.1–5.4) ───────────────────────────

    /// Send a user event to a session.
    ///
    /// POSTs to `/sessions/{id}/events` with the serialized `UserEvent` payload.
    /// Uses automatic retry with exponential backoff for transient errors.
    ///
    /// # Errors
    ///
    /// - `EnterpriseError::NotFound` — if the session does not exist
    /// - `EnterpriseError::Conflict` — if the session is in a terminal state
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::{EnterpriseClient, UserEvent};
    ///
    /// let client = EnterpriseClient::new("adk_live_...")?;
    /// client.send_event("ses_abc123", UserEvent::message("Hello!")).await?;
    /// ```
    pub async fn send_event(&self, session_id: &str, event: UserEvent) -> Result<()> {
        let url = self.build_url(&format!("/sessions/{session_id}/events"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let body = serde_json::to_vec(&event)?;

        let response = execute_with_retry(&policy, || {
            let url = url.clone();
            let headers = headers.clone();
            let body = body.clone();
            async move {
                reqwest::Client::new()
                    .post(&url)
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
            }
        })
        .await?;

        handle_empty_response(response).await
    }

    /// Send a text message to a session.
    ///
    /// Convenience method that wraps the text in a `user.message` event
    /// and calls [`send_event`](Self::send_event).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.send_message("ses_abc123", "What is 2+2?").await?;
    /// ```
    pub async fn send_message(&self, session_id: &str, text: impl Into<String>) -> Result<()> {
        self.send_event(session_id, UserEvent::message(text)).await
    }

    /// Interrupt the agent's current turn.
    ///
    /// Convenience method that sends a `user.interrupt` event.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.interrupt("ses_abc123").await?;
    /// ```
    pub async fn interrupt(&self, session_id: &str) -> Result<()> {
        self.send_event(session_id, UserEvent::interrupt()).await
    }

    /// Allow a pending tool use.
    ///
    /// Sends a `user.tool_confirmation` event with the "allow" action.
    /// The `tool_use_id` must match the event ID from a blocking
    /// `SessionEvent::ToolUse` or `SessionEvent::CustomToolUse` event.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.allow_tool("ses_abc123", "tu_xyz789").await?;
    /// ```
    pub async fn allow_tool(&self, session_id: &str, tool_use_id: &str) -> Result<()> {
        self.send_event(session_id, UserEvent::allow_tool(tool_use_id)).await
    }

    /// Deny a pending tool use with a reason.
    ///
    /// Sends a `user.tool_confirmation` event with the "deny" action and
    /// a user-provided reason explaining why the tool use was denied.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.deny_tool("ses_abc123", "tu_xyz789", "Not authorized to delete files").await?;
    /// ```
    pub async fn deny_tool(
        &self,
        session_id: &str,
        tool_use_id: &str,
        reason: impl Into<String>,
    ) -> Result<()> {
        self.send_event(session_id, UserEvent::deny_tool(tool_use_id, reason)).await
    }

    /// Provide a custom tool result.
    ///
    /// Sends a `user.custom_tool_result` event with the tool execution result.
    /// Used when the agent invokes a custom tool that the client must execute
    /// locally and return the result.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::ContentBlock;
    ///
    /// // Agent requested get_weather tool...
    /// let result = vec![ContentBlock::text("22°C, sunny in Tokyo")];
    /// client.custom_tool_result("ses_abc123", "ctu_xyz789", result).await?;
    /// ```
    pub async fn custom_tool_result(
        &self,
        session_id: &str,
        custom_tool_use_id: &str,
        content: Vec<crate::types::events::ContentBlock>,
    ) -> Result<()> {
        self.send_event(session_id, UserEvent::custom_tool_result(custom_tool_use_id, content))
            .await
    }

    /// Define success criteria for the session outcome.
    ///
    /// Sends a `user.define_outcome` event that tells the agent what
    /// constitutes a successful completion of the current task.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// client.define_outcome(
    ///     "ses_abc123",
    ///     "The PR should be merged and all tests passing",
    /// ).await?;
    /// ```
    pub async fn define_outcome(
        &self,
        session_id: &str,
        criteria: impl Into<String>,
    ) -> Result<()> {
        self.send_event(session_id, UserEvent::define_outcome(criteria)).await
    }

    // ─── Event History (Requirements 7.1, 7.2) ──────────────────────────

    /// List stored events for a session with optional cursor pagination.
    ///
    /// GETs `/sessions/{session_id}/events` with optional `limit` and `cursor`
    /// query parameters. Returns a paginated `ListResponse<StoredEvent>` containing
    /// the event history (both user and agent events) in sequence order.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to list events for (e.g., `"ses_abc123"`)
    /// * `params` - Optional pagination parameters (limit, cursor)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::ListParams;
    ///
    /// let response = client.list_events("ses_abc123", Some(ListParams {
    ///     limit: Some(20),
    ///     cursor: None,
    /// })).await?;
    ///
    /// for event in &response.data {
    ///     println!("  seq={} dir={:?} at={}", event.seq, event.direction, event.created_at);
    /// }
    ///
    /// if response.has_more {
    ///     // Use response.next_cursor for next page
    /// }
    /// ```
    pub async fn list_events(
        &self,
        session_id: &str,
        params: Option<ListParams>,
    ) -> Result<ListResponse<StoredEvent>> {
        let url = self.build_url(&format!("/sessions/{session_id}/events"));
        let headers = self.default_headers();
        let policy = RetryPolicy::from_config(self.config.max_retries, self.config.retry_backoff);

        let query_params = build_events_query_params(params.as_ref());

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
}

/// Build query parameters for the list events endpoint from optional `ListParams`.
fn build_events_query_params(params: Option<&ListParams>) -> Vec<(String, String)> {
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
    use crate::types::events::{ContentBlock, UserEvent};

    #[test]
    fn test_user_event_message_serialization() {
        let event = UserEvent::message("Hello, agent!");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.message""#));
        assert!(json.contains(r#""text":"Hello, agent!""#));
    }

    #[test]
    fn test_user_event_interrupt_serialization() {
        let event = UserEvent::interrupt();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.interrupt""#));
    }

    #[test]
    fn test_user_event_allow_tool_serialization() {
        let event = UserEvent::allow_tool("tu_123");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.tool_confirmation""#));
        assert!(json.contains(r#""tool_use_id":"tu_123""#));
        assert!(json.contains(r#""result":"allow""#));
    }

    #[test]
    fn test_user_event_deny_tool_serialization() {
        let event = UserEvent::deny_tool("tu_456", "Not safe to execute");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.tool_confirmation""#));
        assert!(json.contains(r#""tool_use_id":"tu_456""#));
        assert!(json.contains(r#""result":"deny""#));
        assert!(json.contains(r#""deny_message":"Not safe to execute""#));
    }

    #[test]
    fn test_user_event_custom_tool_result_serialization() {
        let event = UserEvent::custom_tool_result("ctu_789", vec![ContentBlock::text("42°C, hot")]);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.custom_tool_result""#));
        assert!(json.contains(r#""custom_tool_use_id":"ctu_789""#));
    }

    #[test]
    fn test_user_event_define_outcome_serialization() {
        let event = UserEvent::define_outcome("All tests pass");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"user.define_outcome""#));
        assert!(json.contains(r#""criteria":"All tests pass""#));
    }
}
