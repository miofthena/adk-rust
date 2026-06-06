//! End-to-end tests for the retry policy using wiremock.
//!
//! Validates Requirements 9.1–9.6:
//! - 9.1: Retry on 429, 500, 502, 503, 504 responses
//! - 9.2: Exponential backoff with jitter
//! - 9.3: Respect `Retry-After` header on 429
//! - 9.4: Never retry 400, 401, 403, 404, 409, 422
//! - 9.5: Configurable max retries
//! - 9.6: Return last error after exhausting retries

use std::time::Duration;

use adk_enterprise::{ClientConfig, EnterpriseClient, EnterpriseError};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// Helper to create a client pointing at the mock server with fast retry settings.
fn mock_client_with_retries(base_url: &str, max_retries: u32) -> EnterpriseClient {
    let config = ClientConfig::new("adk_live_test_key")
        .with_base_url(base_url)
        .with_max_retries(max_retries)
        .with_retry_backoff(Duration::from_millis(10));
    EnterpriseClient::with_config(config).unwrap()
}

/// Sample agent JSON response.
fn sample_agent_json() -> serde_json::Value {
    serde_json::json!({
        "id": "agt_retry_test",
        "name": "Retry Agent",
        "model": "gemini-2.5-flash",
        "system": null,
        "description": null,
        "tools": [],
        "mcp_servers": [],
        "skills": [],
        "permission_policy": null,
        "metadata": null,
        "version": 1,
        "created_at": "2026-01-15T10:00:00Z",
        "updated_at": "2026-01-15T10:00:00Z",
        "archived_at": null
    })
}

/// A responder that returns different responses on successive calls.
/// First N calls return the "fail" response, then all subsequent calls return "success".
struct SequentialResponder {
    fail_responses: Vec<ResponseTemplate>,
    success_response: ResponseTemplate,
    call_count: std::sync::atomic::AtomicUsize,
}

impl SequentialResponder {
    fn new(fail_responses: Vec<ResponseTemplate>, success_response: ResponseTemplate) -> Self {
        Self {
            fail_responses,
            success_response,
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Respond for SequentialResponder {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        let idx = self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if idx < self.fail_responses.len() {
            self.fail_responses[idx].clone()
        } else {
            self.success_response.clone()
        }
    }
}

// ─── Test: 429 with Retry-After → waits and retries successfully ─────────────

#[tokio::test]
async fn test_429_with_retry_after_retries_and_succeeds() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    // First request returns 429 with Retry-After header, second returns 200
    let responder = SequentialResponder::new(
        vec![ResponseTemplate::new(429).append_header("Retry-After", "1").set_body_json(
            serde_json::json!({
                "error": {
                    "type": "rate_limit_error",
                    "message": "Rate limited"
                }
            }),
        )],
        ResponseTemplate::new(200).set_body_json(sample_agent_json()),
    );

    Mock::given(method("GET"))
        .and(path("/agents/agt_retry_test"))
        .respond_with(responder)
        .expect(2) // initial + 1 retry
        .mount(&server)
        .await;

    let start = std::time::Instant::now();
    let agent = client.get_agent("agt_retry_test").await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(agent.id, "agt_retry_test");
    // Should have waited at least the backoff duration (10ms min from config)
    // The Retry-After says 1s, but our backoff is 10ms - the max is used.
    // With retry_backoff=10ms and Retry-After=1, it should wait at least 1s.
    assert!(
        elapsed >= Duration::from_millis(900),
        "Expected at least ~1s wait for Retry-After, got {elapsed:?}"
    );
}

// ─── Test: 500 → retries up to max and succeeds ─────────────────────────────

#[tokio::test]
async fn test_500_retries_and_succeeds_on_third_attempt() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    // First two requests return 500, third returns 200
    let responder = SequentialResponder::new(
        vec![
            ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": {
                    "type": "internal_error",
                    "message": "Internal server error"
                }
            })),
            ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": {
                    "type": "internal_error",
                    "message": "Internal server error"
                }
            })),
        ],
        ResponseTemplate::new(200).set_body_json(sample_agent_json()),
    );

    Mock::given(method("GET"))
        .and(path("/agents/agt_retry_test"))
        .respond_with(responder)
        .expect(3) // initial + 2 retries
        .mount(&server)
        .await;

    let agent = client.get_agent("agt_retry_test").await.unwrap();
    assert_eq!(agent.id, "agt_retry_test");
}

// ─── Test: 400 → no retry, immediate error ──────────────────────────────────

#[tokio::test]
async fn test_400_no_retry_immediate_error() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    let error_body = serde_json::json!({
        "error": {
            "type": "invalid_request_error",
            "message": "Invalid request parameters",
            "param": "model"
        }
    });

    Mock::given(method("GET"))
        .and(path("/agents/agt_bad"))
        .respond_with(ResponseTemplate::new(400).set_body_json(error_body))
        .expect(1) // Only 1 request — no retries
        .mount(&server)
        .await;

    let result = client.get_agent("agt_bad").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::InvalidRequest { message, .. } => {
            assert_eq!(message, "Invalid request parameters");
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}

#[tokio::test]
async fn test_401_no_retry_immediate_error() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    let error_body = serde_json::json!({
        "error": {
            "type": "authentication_error",
            "message": "Invalid API key"
        }
    });

    Mock::given(method("GET"))
        .and(path("/agents/agt_auth"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.get_agent("agt_auth").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::Authentication { message } => {
            assert_eq!(message, "Invalid API key");
        }
        other => panic!("expected Authentication, got {other:?}"),
    }
}

#[tokio::test]
async fn test_404_no_retry_immediate_error() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    let error_body = serde_json::json!({
        "error": {
            "type": "not_found",
            "message": "Agent not found"
        }
    });

    Mock::given(method("GET"))
        .and(path("/agents/agt_missing"))
        .respond_with(ResponseTemplate::new(404).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.get_agent("agt_missing").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::NotFound { message } => {
            assert_eq!(message, "Agent not found");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

// ─── Test: exhaust retries → returns last error ─────────────────────────────

#[tokio::test]
async fn test_exhaust_retries_returns_last_error() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    let error_body = serde_json::json!({
        "error": {
            "type": "internal_error",
            "message": "Internal server error"
        }
    });

    // All requests return 500 — retries will be exhausted
    Mock::given(method("GET"))
        .and(path("/agents/agt_always_fails"))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body))
        .expect(3) // initial + 2 retries = 3 total
        .mount(&server)
        .await;

    let result = client.get_agent("agt_always_fails").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::Internal { message } => {
            assert_eq!(message, "Internal server error");
        }
        other => panic!("expected Internal, got {other:?}"),
    }
}

#[tokio::test]
async fn test_exhaust_retries_with_503_returns_unavailable() {
    let server = MockServer::start().await;
    let client = mock_client_with_retries(&server.uri(), 2);

    let error_body = serde_json::json!({
        "error": {
            "type": "unavailable",
            "message": "Service temporarily unavailable"
        }
    });

    // All requests return 503
    Mock::given(method("GET"))
        .and(path("/agents/agt_unavailable"))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body))
        .expect(3) // initial + 2 retries
        .mount(&server)
        .await;

    let result = client.get_agent("agt_unavailable").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::Unavailable { message, .. } => {
            assert_eq!(message, "Service temporarily unavailable");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}
