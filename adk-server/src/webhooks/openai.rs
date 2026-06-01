//! OpenAI webhook event handler.
//!
//! Receives signed webhook events from OpenAI for background task completion
//! and other asynchronous notifications. Validates HMAC-SHA256 signatures,
//! parses event payloads, and broadcasts parsed events to subscribers.
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_server::webhooks::{OpenAIWebhookConfig, OpenAIWebhookHandler};
//!
//! let config = OpenAIWebhookConfig {
//!     webhook_secret: "whsec_abc123".to_string(),
//!     path: "/webhooks/openai".to_string(),
//! };
//!
//! let handler = OpenAIWebhookHandler::new(config);
//! let mut rx = handler.subscribe();
//! let router = handler.router();
//!
//! // Mount router in your Axum app
//! // Events will be delivered via rx
//! ```

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Configuration for the OpenAI webhook handler.
#[derive(Debug, Clone)]
pub struct OpenAIWebhookConfig {
    /// Webhook signing secret for HMAC-SHA256 validation.
    pub webhook_secret: String,
    /// Path to mount the webhook endpoint (e.g., "/webhooks/openai").
    pub path: String,
}

/// A parsed webhook event from OpenAI.
#[derive(Debug, Clone)]
pub struct WebhookEvent {
    /// The type of event (e.g., "response.completed", "response.failed").
    pub event_type: String,
    /// The response ID associated with this event.
    pub response_id: String,
    /// The raw response payload as JSON, if present.
    /// Consumers with access to `adk-model` can convert this to `LlmResponse`
    /// via `responses_convert::from_response`.
    pub response: Option<serde_json::Value>,
    /// Error information, if the event represents a failure.
    pub error: Option<String>,
}

/// OpenAI webhook event handler.
///
/// Provides an Axum router that receives POST requests at the configured path,
/// validates HMAC-SHA256 signatures, parses event payloads, and broadcasts
/// parsed events to subscribers via a tokio broadcast channel.
#[derive(Clone)]
pub struct OpenAIWebhookHandler {
    config: Arc<OpenAIWebhookConfig>,
    tx: broadcast::Sender<WebhookEvent>,
}

impl OpenAIWebhookHandler {
    /// Create a new webhook handler with the given configuration.
    ///
    /// The broadcast channel has a capacity of 64 events. If subscribers
    /// fall behind, older events will be dropped.
    pub fn new(config: OpenAIWebhookConfig) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self { config: Arc::new(config), tx }
    }

    /// Create an Axum router for the webhook endpoint.
    ///
    /// The router handles POST requests at the configured path.
    /// It validates the HMAC-SHA256 signature from the `X-OpenAI-Signature`
    /// header, returning HTTP 401 for invalid signatures and HTTP 400 for
    /// parse errors.
    pub fn router(&self) -> Router {
        let handler_state = self.clone();
        Router::new().route(&self.config.path, post(handle_webhook).with_state(handler_state))
    }

    /// Subscribe to webhook events.
    ///
    /// Returns a broadcast receiver that will receive all parsed webhook events.
    /// Multiple subscribers can be active simultaneously.
    pub fn subscribe(&self) -> broadcast::Receiver<WebhookEvent> {
        self.tx.subscribe()
    }

    /// Validate HMAC-SHA256 signature of a webhook payload.
    ///
    /// The signature is expected to be a hex-encoded HMAC-SHA256 digest
    /// computed using the webhook secret.
    pub fn validate_signature(&self, payload: &[u8], signature: &str) -> bool {
        type HmacSha256 = Hmac<Sha256>;

        let Ok(mut mac) = HmacSha256::new_from_slice(self.config.webhook_secret.as_bytes()) else {
            warn!("invalid webhook secret length for HMAC");
            return false;
        };

        mac.update(payload);

        // Try hex-encoded signature
        let Ok(signature_bytes) = hex::decode(signature) else {
            debug!("webhook signature is not valid hex");
            return false;
        };

        mac.verify_slice(&signature_bytes).is_ok()
    }
}

/// The header name for the OpenAI webhook signature.
const SIGNATURE_HEADER: &str = "x-openai-signature";

/// Axum handler for incoming webhook POST requests.
async fn handle_webhook(
    State(handler): State<OpenAIWebhookHandler>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Extract signature from headers
    let signature = match headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok()) {
        Some(sig) => sig,
        None => {
            warn!("webhook request missing signature header");
            return StatusCode::UNAUTHORIZED;
        }
    };

    // Validate HMAC-SHA256 signature
    if !handler.validate_signature(&body, signature) {
        warn!("webhook signature validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    // Parse the JSON payload
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("webhook payload parse error: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    // Extract event_type and response_id
    let event_type = match payload.get("type").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            warn!("webhook payload missing 'type' field");
            return StatusCode::BAD_REQUEST;
        }
    };

    let response_id = payload
        .get("response")
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("response_id").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    // Extract response data for completed events
    let response = payload.get("response").cloned();

    // Extract error information for failed events
    let error = payload
        .get("error")
        .or_else(|| payload.get("response").and_then(|r| r.get("error")))
        .map(|e| e.to_string());

    let event = WebhookEvent { event_type, response_id, response, error };

    debug!(
        event_type = %event.event_type,
        response_id = %event.response_id,
        "webhook event received"
    );

    // Broadcast to subscribers (ignore send errors — no subscribers is fine)
    let _ = handler.tx.send(event);

    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_signature_valid() {
        let config = OpenAIWebhookConfig {
            webhook_secret: "test_secret".to_string(),
            path: "/webhooks/openai".to_string(),
        };
        let handler = OpenAIWebhookHandler::new(config);

        let payload = b"hello world";

        // Compute expected signature
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
        mac.update(payload);
        let expected = hex::encode(mac.finalize().into_bytes());

        assert!(handler.validate_signature(payload, &expected));
    }

    #[test]
    fn test_validate_signature_invalid() {
        let config = OpenAIWebhookConfig {
            webhook_secret: "test_secret".to_string(),
            path: "/webhooks/openai".to_string(),
        };
        let handler = OpenAIWebhookHandler::new(config);

        let payload = b"hello world";
        let bad_signature = "deadbeef0000000000000000000000000000000000000000000000000000000000";

        assert!(!handler.validate_signature(payload, bad_signature));
    }

    #[test]
    fn test_validate_signature_invalid_hex() {
        let config = OpenAIWebhookConfig {
            webhook_secret: "test_secret".to_string(),
            path: "/webhooks/openai".to_string(),
        };
        let handler = OpenAIWebhookHandler::new(config);

        assert!(!handler.validate_signature(b"payload", "not-valid-hex!@#$"));
    }

    #[test]
    fn test_subscribe_returns_receiver() {
        let config = OpenAIWebhookConfig {
            webhook_secret: "secret".to_string(),
            path: "/webhooks/openai".to_string(),
        };
        let handler = OpenAIWebhookHandler::new(config);

        let _rx1 = handler.subscribe();
        let _rx2 = handler.subscribe();
        // Multiple subscribers should work
    }

    #[tokio::test]
    async fn test_broadcast_event() {
        let config = OpenAIWebhookConfig {
            webhook_secret: "secret".to_string(),
            path: "/webhooks/openai".to_string(),
        };
        let handler = OpenAIWebhookHandler::new(config);

        let mut rx = handler.subscribe();

        let event = WebhookEvent {
            event_type: "response.completed".to_string(),
            response_id: "resp_123".to_string(),
            response: Some(serde_json::json!({"id": "resp_123", "status": "completed"})),
            error: None,
        };

        handler.tx.send(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, "response.completed");
        assert_eq!(received.response_id, "resp_123");
    }
}
