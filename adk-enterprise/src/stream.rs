//! EventStream — resilient SSE stream with auto-reconnect.
//!
//! Implements Requirements 6.1–6.8:
//! - `stream_events(session_id)` opens GET `/sessions/{id}/events/stream`
//! - `EventStream` implements `futures::Stream<Item = Result<SessionEvent>>`
//! - SSE frame parsing: buffer bytes, split on `\n\n`, extract `event:`, `data:`, `id:` fields
//! - Auto-reconnect on disconnect/timeout using `Last-Event-ID: {last_seq}`
//! - Unknown event types → `SessionEvent::Unknown`
//! - Invalid JSON → log warning, skip, continue stream
//! - Configurable max reconnect attempts before terminal error

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_stream::stream;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::Stream;
use reqwest::header::{ACCEPT, HeaderValue};
use tracing::{debug, warn};

use crate::client::EnterpriseClient;
use crate::error::EnterpriseError;
use crate::types::events::SessionEvent;

/// Default maximum reconnect attempts before yielding a terminal error.
const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Brief delay before reconnecting after a connection drop (milliseconds).
const RECONNECT_DELAY_MS: u64 = 500;

// ─── SSE Frame Parser (Task 7.1) ─────────────────────────────────────────────

/// A parsed SSE frame with its extracted fields.
#[derive(Debug, Default)]
struct SseFrame {
    /// The `event:` field value (optional).
    event: Option<String>,
    /// The `data:` field value (multi-line data concatenated with newlines).
    data: Option<String>,
    /// The `id:` field value (used for reconnection via `Last-Event-ID`).
    id: Option<String>,
}

/// Parses a complete SSE event block (text between `\n\n` boundaries) into an `SseFrame`.
///
/// Per the SSE spec:
/// - Lines starting with `:` are comments and skipped
/// - `event:` sets the event type
/// - `data:` appends to the data buffer (multi-line data is joined with `\n`)
/// - `id:` sets the last event ID
/// - Empty lines are the event terminator (already handled by the caller splitting on `\n\n`)
fn parse_sse_block(block: &str) -> Option<SseFrame> {
    let mut frame = SseFrame::default();
    let mut has_data = false;

    for line in block.lines() {
        // Skip empty lines within a block
        if line.is_empty() {
            continue;
        }

        // Skip SSE comments (lines starting with `:`)
        if line.starts_with(':') {
            continue;
        }

        if let Some(value) = line.strip_prefix("data:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if has_data {
                // Multi-line data: concatenate with newline
                if let Some(ref mut existing) = frame.data {
                    existing.push('\n');
                    existing.push_str(value);
                }
            } else {
                frame.data = Some(value.to_string());
                has_data = true;
            }
        } else if let Some(value) = line.strip_prefix("event:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            frame.event = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("id:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if !value.is_empty() {
                frame.id = Some(value.to_string());
            }
        }
        // Unknown fields are ignored per SSE spec
    }

    // Only return a frame if we have data
    if has_data { Some(frame) } else { None }
}

/// Deserializes the `data:` JSON payload into a `SessionEvent`.
///
/// - Valid JSON → corresponding `SessionEvent` variant
/// - Unknown event type in JSON → `SessionEvent::Unknown` (via `#[serde(other)]`)
/// - Invalid JSON → `None` (caller logs warning and skips)
fn deserialize_event(data: &str) -> Option<SessionEvent> {
    match serde_json::from_str::<SessionEvent>(data) {
        Ok(event) => Some(event),
        Err(e) => {
            warn!(error = %e, data_preview = &data[..data.len().min(100)], "invalid JSON in SSE data, skipping");
            None
        }
    }
}

// ─── EventStream Wrapper (Task 7.3) ──────────────────────────────────────────

/// A resilient event stream that auto-reconnects on disconnect.
///
/// Uses the `id:` field from SSE frames (which carries the `seq`) to
/// reconnect from the last received event via `Last-Event-ID` header.
///
/// Implements `futures::Stream<Item = Result<SessionEvent>>` directly for
/// ergonomic use with `StreamExt` combinators.
///
/// # Example
///
/// ```rust,ignore
/// use futures::StreamExt;
///
/// let mut stream = client.stream_events("ses_abc123").await?;
///
/// while let Some(event) = stream.next().await {
///     match event? {
///         SessionEvent::Message { content, .. } => println!("{content:?}"),
///         SessionEvent::StatusIdle { .. } => break,
///         _ => {}
///     }
/// }
/// // Dropping `stream` cancels the underlying connection.
/// ```
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = crate::Result<SessionEvent>> + Send>>,
}

impl EventStream {
    /// Create a new `EventStream` wrapping an inner stream.
    pub(crate) fn new(
        inner: Pin<Box<dyn Stream<Item = crate::Result<SessionEvent>> + Send>>,
    ) -> Self {
        Self { inner }
    }

    /// Convert into the underlying stream for explicit use.
    pub fn into_stream(self) -> impl Stream<Item = crate::Result<SessionEvent>> + Send {
        self.inner
    }
}

impl Stream for EventStream {
    type Item = crate::Result<SessionEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

// ─── Auto-Reconnect Logic (Task 7.2) + stream_events() (Task 7.4) ────────────

impl EnterpriseClient {
    /// Open a real-time SSE event stream for a session.
    ///
    /// Opens a GET connection to `/sessions/{id}/events/stream` with
    /// `Accept: text/event-stream` header. Returns an `EventStream` that
    /// yields `SessionEvent` items.
    ///
    /// The stream automatically reconnects on connection drops or timeouts,
    /// using the `Last-Event-ID` header to resume from the last received event.
    /// This is transparent to the consumer — no gap in the yielded stream.
    ///
    /// # Errors
    ///
    /// Returns an error immediately if the initial connection fails (e.g., 404, 401).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_enterprise::{EnterpriseClient, SessionEvent};
    /// use futures::StreamExt;
    ///
    /// let client = EnterpriseClient::from_env()?;
    /// let mut stream = client.stream_events("ses_abc123").await?;
    ///
    /// while let Some(event) = stream.next().await {
    ///     match event? {
    ///         SessionEvent::Message { content, .. } => {
    ///             println!("Agent says: {content:?}");
    ///         }
    ///         SessionEvent::StatusIdle { .. } => break,
    ///         _ => {}
    ///     }
    /// }
    /// ```
    pub async fn stream_events(&self, session_id: &str) -> crate::Result<EventStream> {
        let url = self.build_url(&format!("/sessions/{session_id}/events/stream"));
        let sse_timeout = self.config.sse_timeout;

        // Attempt the initial connection to fail fast on auth/not-found errors
        let response = self.open_sse_connection(&url, None).await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(map_sse_connect_error(status, &body));
        }

        // Build the auto-reconnecting stream
        let client = self.clone();
        let url_clone = url.clone();

        let event_stream = stream! {
            let mut last_event_id: Option<String> = None;
            let mut reconnect_attempts: u32 = 0;
            let mut byte_stream: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>> =
                Box::pin(response.bytes_stream());
            let mut buffer = String::new();

            loop {
                // Read from the byte stream with timeout
                let chunk_result = tokio::time::timeout(
                    sse_timeout,
                    byte_stream.next(),
                ).await;

                match chunk_result {
                    // Timeout — no data within sse_timeout, reconnect
                    Err(_elapsed) => {
                        debug!(
                            timeout_secs = sse_timeout.as_secs(),
                            last_event_id = ?last_event_id,
                            "SSE stream timeout, reconnecting"
                        );
                        match client.reconnect_sse(
                            &url_clone,
                            &last_event_id,
                            &mut reconnect_attempts,
                        ).await {
                            Ok(new_response) => {
                                byte_stream = Box::pin(new_response.bytes_stream());
                                buffer.clear();
                                reconnect_attempts = 0;
                                continue;
                            }
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }
                    // Stream ended (None) — connection dropped, reconnect
                    Ok(None) => {
                        debug!(
                            last_event_id = ?last_event_id,
                            "SSE connection dropped, reconnecting"
                        );
                        match client.reconnect_sse(
                            &url_clone,
                            &last_event_id,
                            &mut reconnect_attempts,
                        ).await {
                            Ok(new_response) => {
                                byte_stream = Box::pin(new_response.bytes_stream());
                                buffer.clear();
                                reconnect_attempts = 0;
                                continue;
                            }
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }
                    // Received bytes — process them
                    Ok(Some(Ok(bytes))) => {
                        // Reset reconnect attempts on successful data receipt
                        reconnect_attempts = 0;

                        let text = match std::str::from_utf8(&bytes) {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(error = %e, "non-UTF8 data in SSE stream, skipping chunk");
                                continue;
                            }
                        };

                        buffer.push_str(text);

                        // Process complete events (terminated by \n\n)
                        while let Some(split_pos) = buffer.find("\n\n") {
                            let block = buffer[..split_pos].to_string();
                            buffer = buffer[split_pos + 2..].to_string();

                            if block.trim().is_empty() {
                                continue;
                            }

                            if let Some(frame) = parse_sse_block(&block) {
                                // Track the id field for reconnection
                                if let Some(ref id) = frame.id {
                                    last_event_id = Some(id.clone());
                                }

                                // Deserialize the data payload
                                // Invalid JSON is logged and skipped (handled inside deserialize_event)
                                if let Some(ref data) = frame.data
                                    && let Some(event) = deserialize_event(data)
                                {
                                    yield Ok(event);
                                }
                            }
                        }
                    }
                    // Network error reading bytes — attempt reconnect
                    Ok(Some(Err(e))) => {
                        warn!(error = %e, "SSE byte stream error, attempting reconnect");
                        match client.reconnect_sse(
                            &url_clone,
                            &last_event_id,
                            &mut reconnect_attempts,
                        ).await {
                            Ok(new_response) => {
                                byte_stream = Box::pin(new_response.bytes_stream());
                                buffer.clear();
                                reconnect_attempts = 0;
                                continue;
                            }
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }
                }
            }
        };

        Ok(EventStream::new(Box::pin(event_stream)))
    }

    /// Open an SSE connection to the given URL, optionally with `Last-Event-ID`.
    async fn open_sse_connection(
        &self,
        url: &str,
        last_event_id: Option<&str>,
    ) -> crate::Result<reqwest::Response> {
        let mut headers = self.default_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        // Remove Content-Type for GET SSE request (no body)
        headers.remove(reqwest::header::CONTENT_TYPE);

        if let Some(id) = last_event_id {
            headers.insert(
                "Last-Event-ID",
                HeaderValue::from_str(id).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }

        let response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(EnterpriseError::Connection)?;

        Ok(response)
    }

    /// Attempt to reconnect to the SSE stream with exponential delay.
    ///
    /// Increments `attempts` and returns an error if max attempts exceeded.
    /// On success, returns the new response for streaming.
    async fn reconnect_sse(
        &self,
        url: &str,
        last_event_id: &Option<String>,
        attempts: &mut u32,
    ) -> crate::Result<reqwest::Response> {
        *attempts += 1;

        if *attempts > DEFAULT_MAX_RECONNECT_ATTEMPTS {
            return Err(EnterpriseError::Stream {
                message: format!(
                    "SSE reconnection failed after {DEFAULT_MAX_RECONNECT_ATTEMPTS} attempts"
                ),
            });
        }

        // Wait with brief exponential backoff before reconnecting
        let delay = Duration::from_millis(RECONNECT_DELAY_MS * (*attempts as u64));
        debug!(
            attempt = *attempts,
            max_attempts = DEFAULT_MAX_RECONNECT_ATTEMPTS,
            delay_ms = delay.as_millis() as u64,
            last_event_id = ?last_event_id,
            "reconnecting SSE stream"
        );
        tokio::time::sleep(delay).await;

        let response = self.open_sse_connection(url, last_event_id.as_deref()).await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(map_sse_connect_error(status, &body));
        }

        Ok(response)
    }
}

/// Map an HTTP error status from SSE connection to the appropriate `EnterpriseError`.
fn map_sse_connect_error(status: reqwest::StatusCode, body: &str) -> EnterpriseError {
    let message = if body.is_empty() {
        format!("SSE connection failed with status {status}")
    } else {
        // Try to extract a message from a JSON error body
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .unwrap_or_else(|| format!("SSE connection failed with status {status}"))
    };

    match status.as_u16() {
        401 => EnterpriseError::Authentication { message },
        403 => EnterpriseError::Permission { message },
        404 => EnterpriseError::NotFound { message },
        409 => EnterpriseError::Conflict { message },
        429 => EnterpriseError::RateLimit { message, retry_after: None },
        500 => EnterpriseError::Internal { message },
        503 => EnterpriseError::Unavailable { message, retry_after: None },
        _ => EnterpriseError::Stream { message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SSE Frame Parser Tests ──────────────────────────────────────────

    #[test]
    fn test_parse_simple_event() {
        let block = "data: {\"type\":\"status.running\",\"seq\":1}";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.data.as_deref(), Some("{\"type\":\"status.running\",\"seq\":1}"));
        assert!(frame.event.is_none());
        assert!(frame.id.is_none());
    }

    #[test]
    fn test_parse_event_with_all_fields() {
        let block =
            "event: message\nid: 42\ndata: {\"type\":\"agent.message\",\"seq\":42,\"content\":[]}";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.event.as_deref(), Some("message"));
        assert_eq!(frame.id.as_deref(), Some("42"));
        assert_eq!(
            frame.data.as_deref(),
            Some("{\"type\":\"agent.message\",\"seq\":42,\"content\":[]}")
        );
    }

    #[test]
    fn test_parse_multiline_data() {
        let block = "data: line1\ndata: line2\ndata: line3";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.data.as_deref(), Some("line1\nline2\nline3"));
    }

    #[test]
    fn test_parse_comment_lines_skipped() {
        let block =
            ": this is a comment\ndata: {\"type\":\"status.running\",\"seq\":1}\n: another comment";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.data.as_deref(), Some("{\"type\":\"status.running\",\"seq\":1}"));
    }

    #[test]
    fn test_parse_empty_block_returns_none() {
        let block = "";
        assert!(parse_sse_block(block).is_none());
    }

    #[test]
    fn test_parse_comment_only_block_returns_none() {
        let block = ": keepalive\n: ping";
        assert!(parse_sse_block(block).is_none());
    }

    #[test]
    fn test_parse_data_with_leading_space() {
        let block = "data: hello world";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.data.as_deref(), Some("hello world"));
    }

    #[test]
    fn test_parse_data_without_leading_space() {
        let block = "data:hello world";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.data.as_deref(), Some("hello world"));
    }

    #[test]
    fn test_parse_empty_id_field_ignored() {
        let block = "id:\ndata: test";
        let frame = parse_sse_block(block).unwrap();
        assert!(frame.id.is_none());
        assert_eq!(frame.data.as_deref(), Some("test"));
    }

    #[test]
    fn test_parse_id_with_value() {
        let block = "id: 99\ndata: test";
        let frame = parse_sse_block(block).unwrap();
        assert_eq!(frame.id.as_deref(), Some("99"));
    }

    // ─── Deserialization Tests ───────────────────────────────────────────

    #[test]
    fn test_deserialize_valid_message_event() {
        let data =
            r#"{"type":"agent.message","seq":5,"content":[{"type":"text","text":"Hello!"}]}"#;
        let event = deserialize_event(data).unwrap();
        match event {
            SessionEvent::Message { seq, content } => {
                assert_eq!(seq, 5);
                assert_eq!(content.len(), 1);
            }
            _ => panic!("expected Message event"),
        }
    }

    #[test]
    fn test_deserialize_status_idle_event() {
        let data = r#"{"type":"status.idle","seq":10,"stop_reason":null}"#;
        let event = deserialize_event(data).unwrap();
        match event {
            SessionEvent::StatusIdle { seq, stop_reason, .. } => {
                assert_eq!(seq, 10);
                assert!(stop_reason.is_none());
            }
            _ => panic!("expected StatusIdle event"),
        }
    }

    #[test]
    fn test_deserialize_unknown_event_type() {
        let data = r#"{"type":"some.future.event","seq":99,"payload":"anything"}"#;
        let event = deserialize_event(data).unwrap();
        assert!(matches!(event, SessionEvent::Unknown));
    }

    #[test]
    fn test_deserialize_invalid_json_returns_none() {
        let data = "not valid json at all {{{";
        let event = deserialize_event(data);
        assert!(event.is_none());
    }

    #[test]
    fn test_deserialize_tool_use_event() {
        let data = r#"{"type":"agent.tool_use","seq":3,"tool_use_id":"tu_abc","name":"bash","input":{"command":"ls"}}"#;
        let event = deserialize_event(data).unwrap();
        match event {
            SessionEvent::ToolUse { seq, tool_use_id, name, .. } => {
                assert_eq!(seq, 3);
                assert_eq!(tool_use_id, "tu_abc");
                assert_eq!(name, "bash");
            }
            _ => panic!("expected ToolUse event"),
        }
    }

    #[test]
    fn test_deserialize_error_event() {
        let data =
            r#"{"type":"agent.error","seq":7,"message":"something failed","code":"internal"}"#;
        let event = deserialize_event(data).unwrap();
        match event {
            SessionEvent::Error { seq, message, code } => {
                assert_eq!(seq, 7);
                assert_eq!(message, "something failed");
                assert_eq!(code.as_deref(), Some("internal"));
            }
            _ => panic!("expected Error event"),
        }
    }

    // ─── Error Mapping Tests ─────────────────────────────────────────────

    #[test]
    fn test_map_sse_connect_error_401() {
        let err = map_sse_connect_error(reqwest::StatusCode::UNAUTHORIZED, "");
        assert!(matches!(err, EnterpriseError::Authentication { .. }));
    }

    #[test]
    fn test_map_sse_connect_error_404() {
        let err = map_sse_connect_error(reqwest::StatusCode::NOT_FOUND, "");
        assert!(matches!(err, EnterpriseError::NotFound { .. }));
    }

    #[test]
    fn test_map_sse_connect_error_json_body() {
        let body = r#"{"message":"Session not found"}"#;
        let err = map_sse_connect_error(reqwest::StatusCode::NOT_FOUND, body);
        match err {
            EnterpriseError::NotFound { message } => {
                assert_eq!(message, "Session not found");
            }
            _ => panic!("expected NotFound error"),
        }
    }

    #[test]
    fn test_map_sse_connect_error_unknown_status() {
        let err = map_sse_connect_error(reqwest::StatusCode::IM_A_TEAPOT, "");
        assert!(matches!(err, EnterpriseError::Stream { .. }));
    }

    // ─── Integration-style buffer parsing tests ──────────────────────────

    #[test]
    fn test_buffer_splitting_multiple_events() {
        let raw = "data: {\"type\":\"status.running\",\"seq\":1}\n\ndata: {\"type\":\"agent.message\",\"seq\":2,\"content\":[]}\n\n";

        let events: Vec<_> = raw
            .split("\n\n")
            .filter(|block| !block.trim().is_empty())
            .filter_map(parse_sse_block)
            .filter_map(|frame| frame.data.as_ref().and_then(|d| deserialize_event(d)))
            .collect();

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SessionEvent::StatusRunning { seq: 1 }));
        assert!(matches!(events[1], SessionEvent::Message { seq: 2, .. }));
    }

    #[test]
    fn test_buffer_with_id_tracking() {
        let raw = "id: 1\ndata: {\"type\":\"status.running\",\"seq\":1}\n\nid: 2\ndata: {\"type\":\"status.idle\",\"seq\":2,\"stop_reason\":null}\n\n";

        let mut last_id: Option<String> = None;
        for block in raw.split("\n\n") {
            if block.trim().is_empty() {
                continue;
            }
            if let Some(frame) = parse_sse_block(block) {
                if let Some(ref id) = frame.id {
                    last_id = Some(id.clone());
                }
            }
        }

        assert_eq!(last_id.as_deref(), Some("2"));
    }

    #[test]
    fn test_buffer_with_keepalive_comments() {
        let raw = ": keepalive\n\ndata: {\"type\":\"status.running\",\"seq\":1}\n\n: ping\n\n";

        let events: Vec<_> = raw
            .split("\n\n")
            .filter(|block| !block.trim().is_empty())
            .filter_map(parse_sse_block)
            .filter_map(|frame| frame.data.as_ref().and_then(|d| deserialize_event(d)))
            .collect();

        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SessionEvent::StatusRunning { seq: 1 }));
    }
}
