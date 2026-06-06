//! Unit tests for Agent CRUD methods using wiremock mock HTTP server.
//!
//! Tests Requirements 2.1–2.7:
//! - create_agent sends POST /agents with Idempotency-Key header
//! - get_agent sends GET /agents/{id}
//! - list_agents sends GET /agents with query params
//! - update_agent sends PATCH /agents/{id}
//! - archive_agent sends POST /agents/{id}/archive
//! - delete_agent sends DELETE /agents/{id}
//! - Not found returns EnterpriseError::NotFound

use adk_enterprise::{
    ClientConfig, CreateAgentParams, EnterpriseClient, EnterpriseError, ListParams,
    UpdateAgentParams,
};
use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to create a client pointing at the mock server.
fn mock_client(base_url: &str) -> EnterpriseClient {
    let config = ClientConfig::new("adk_live_test_key").with_base_url(base_url).with_max_retries(0); // No retries in tests for speed
    EnterpriseClient::with_config(config).unwrap()
}

/// Sample agent JSON response from the API.
fn sample_agent_json() -> serde_json::Value {
    serde_json::json!({
        "id": "agt_abc123",
        "name": "Test Agent",
        "model": "gemini-2.5-flash",
        "system": "You are helpful.",
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

#[tokio::test]
async fn test_create_agent_posts_to_agents_with_idempotency_key() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    Mock::given(method("POST"))
        .and(path("/agents"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .and(header("Content-Type", "application/json"))
        .and(header("ADK-Managed-Agent", "2026-06-01"))
        .and(header_exists("Idempotency-Key"))
        .respond_with(ResponseTemplate::new(201).set_body_json(sample_agent_json()))
        .expect(1)
        .mount(&server)
        .await;

    let params = CreateAgentParams {
        name: "Test Agent".into(),
        model: "gemini-2.5-flash".into(),
        system: Some("You are helpful.".into()),
        ..Default::default()
    };

    let agent = client.create_agent(params).await.unwrap();

    assert_eq!(agent.id, "agt_abc123");
    assert_eq!(agent.name, "Test Agent");
    assert_eq!(agent.version, 1);
}

#[tokio::test]
async fn test_create_agent_sends_correct_body() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let expected_body = serde_json::json!({
        "name": "My Agent",
        "model": "gpt-4.1",
        "system": "Be concise."
    });

    Mock::given(method("POST"))
        .and(path("/agents"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(201).set_body_json(sample_agent_json()))
        .expect(1)
        .mount(&server)
        .await;

    let params = CreateAgentParams {
        name: "My Agent".into(),
        model: "gpt-4.1".into(),
        system: Some("Be concise.".into()),
        ..Default::default()
    };

    let _ = client.create_agent(params).await.unwrap();
}

#[tokio::test]
async fn test_get_agent_sends_get_to_agents_id() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    Mock::given(method("GET"))
        .and(path("/agents/agt_abc123"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sample_agent_json()))
        .expect(1)
        .mount(&server)
        .await;

    let agent = client.get_agent("agt_abc123").await.unwrap();

    assert_eq!(agent.id, "agt_abc123");
    assert_eq!(agent.name, "Test Agent");
}

#[tokio::test]
async fn test_get_agent_not_found_returns_error() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let error_body = serde_json::json!({
        "error": {
            "type": "not_found",
            "message": "Agent not found",
            "param": "agent_id"
        }
    });

    Mock::given(method("GET"))
        .and(path("/agents/agt_nonexistent"))
        .respond_with(ResponseTemplate::new(404).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.get_agent("agt_nonexistent").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::NotFound { message } => {
            assert_eq!(message, "Agent not found");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn test_list_agents_without_params() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let list_response = serde_json::json!({
        "data": [sample_agent_json()],
        "next_cursor": "cur_xyz",
        "has_more": true
    });

    Mock::given(method("GET"))
        .and(path("/agents"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list_response))
        .expect(1)
        .mount(&server)
        .await;

    let response = client.list_agents(None).await.unwrap();

    assert_eq!(response.data.len(), 1);
    assert_eq!(response.data[0].id, "agt_abc123");
    assert_eq!(response.next_cursor, Some("cur_xyz".into()));
    assert!(response.has_more);
}

#[tokio::test]
async fn test_list_agents_with_pagination_params() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let list_response = serde_json::json!({
        "data": [],
        "has_more": false
    });

    Mock::given(method("GET"))
        .and(path("/agents"))
        .and(query_param("limit", "5"))
        .and(query_param("cursor", "cur_abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list_response))
        .expect(1)
        .mount(&server)
        .await;

    let params = ListParams { limit: Some(5), cursor: Some("cur_abc".into()) };
    let response = client.list_agents(Some(params)).await.unwrap();

    assert!(response.data.is_empty());
    assert!(!response.has_more);
}

#[tokio::test]
async fn test_update_agent_sends_patch() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let mut updated_json = sample_agent_json();
    updated_json["name"] = serde_json::json!("Updated Agent");
    updated_json["version"] = serde_json::json!(2);

    let expected_body = serde_json::json!({
        "name": "Updated Agent"
    });

    Mock::given(method("PATCH"))
        .and(path("/agents/agt_abc123"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(updated_json))
        .expect(1)
        .mount(&server)
        .await;

    let params = UpdateAgentParams { name: Some("Updated Agent".into()), ..Default::default() };

    let agent = client.update_agent("agt_abc123", params).await.unwrap();

    assert_eq!(agent.name, "Updated Agent");
    assert_eq!(agent.version, 2);
}

#[tokio::test]
async fn test_archive_agent_posts_to_archive() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let mut archived_json = sample_agent_json();
    archived_json["archived_at"] = serde_json::json!("2026-01-16T10:00:00Z");

    Mock::given(method("POST"))
        .and(path("/agents/agt_abc123/archive"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(archived_json))
        .expect(1)
        .mount(&server)
        .await;

    let agent = client.archive_agent("agt_abc123").await.unwrap();

    assert_eq!(agent.archived_at, Some("2026-01-16T10:00:00Z".into()));
}

#[tokio::test]
async fn test_delete_agent_sends_delete() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    Mock::given(method("DELETE"))
        .and(path("/agents/agt_abc123"))
        .and(header("Authorization", "Bearer adk_live_test_key"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.delete_agent("agt_abc123").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_delete_agent_not_found() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let error_body = serde_json::json!({
        "error": {
            "type": "not_found",
            "message": "Agent not found"
        }
    });

    Mock::given(method("DELETE"))
        .and(path("/agents/agt_nonexistent"))
        .respond_with(ResponseTemplate::new(404).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let result = client.delete_agent("agt_nonexistent").await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::NotFound { message } => {
            assert_eq!(message, "Agent not found");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn test_create_agent_authentication_error() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let error_body = serde_json::json!({
        "error": {
            "type": "authentication_error",
            "message": "Invalid API key"
        }
    });

    Mock::given(method("POST"))
        .and(path("/agents"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let params = CreateAgentParams {
        name: "Test".into(),
        model: "gemini-2.5-flash".into(),
        ..Default::default()
    };

    let result = client.create_agent(params).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::Authentication { message } => {
            assert_eq!(message, "Invalid API key");
        }
        other => panic!("expected Authentication, got {other:?}"),
    }
}

#[tokio::test]
async fn test_update_agent_validation_error() {
    let server = MockServer::start().await;
    let client = mock_client(&server.uri());

    let error_body = serde_json::json!({
        "error": {
            "type": "validation_error",
            "message": "Invalid model reference",
            "param": "model"
        }
    });

    Mock::given(method("PATCH"))
        .and(path("/agents/agt_abc123"))
        .respond_with(ResponseTemplate::new(422).set_body_json(error_body))
        .expect(1)
        .mount(&server)
        .await;

    let params = UpdateAgentParams { model: Some("invalid-model!!!".into()), ..Default::default() };

    let result = client.update_agent("agt_abc123", params).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        EnterpriseError::Validation { message } => {
            assert_eq!(message, "Invalid model reference");
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}
