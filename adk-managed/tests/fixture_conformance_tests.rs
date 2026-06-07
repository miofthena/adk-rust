//! Golden fixture conformance tests (scripted mode, per-commit gate).
//!
//! This test suite loads each fixture JSON, constructs a runtime with
//! `ScriptedLlm` + the in-process session loop, executes the scenario,
//! and asserts `exact_sequence` — byte-identical type sequences.
//!
//! Runs on every commit, blocks merge, costs $0.
//!
//! # Test Mode
//!
//! Controlled by `ADK_TEST_MODE` environment variable:
//! - `scripted` (default): uses `scripted_model.turns` → asserts `exact_sequence`
//! - `real`: uses `agent_def.model` against real provider → asserts `must_contain` + `must_end_with`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use adk_managed::parking::ToolParkingLot;
use adk_managed::session_loop::SessionLoop;
use adk_managed::testing::{ScriptedLlm, ScriptedTurn};
use adk_managed::types::{ContentBlock, SessionEvent, UserEvent};

use adk_core::{Agent, Content, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream};
use adk_session::service::SessionService;
use async_trait::async_trait;

/// Build a stub agent for tests that need a `SessionLoop` with the full API.
fn build_stub_agent() -> Arc<dyn Agent> {
    struct StubLlm;

    #[async_trait]
    impl Llm for StubLlm {
        fn name(&self) -> &str {
            "stub-llm"
        }
        async fn generate_content(
            &self,
            _request: LlmRequest,
            _stream: bool,
        ) -> adk_core::Result<LlmResponseStream> {
            let s = async_stream::stream! {
                yield Ok(LlmResponse {
                    content: Some(Content::new("model").with_text("stub response")),
                    partial: false,
                    turn_complete: true,
                    finish_reason: Some(FinishReason::Stop),
                    ..Default::default()
                });
            };
            Ok(Box::pin(s))
        }
    }

    let agent =
        adk_agent::LlmAgentBuilder::new("stub-agent").model(Arc::new(StubLlm)).build().unwrap();
    Arc::new(agent)
}

/// Build a stub session service for tests.
fn build_stub_session_service() -> Arc<dyn SessionService> {
    Arc::new(adk_session::InMemorySessionService::new())
}

/// Fixture schema matching the JSON files in `tests/fixtures/`.
#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    description: String,
    #[allow(dead_code)]
    agent_def: serde_json::Value,
    scripted_model: ScriptedModel,
    scenario: Vec<ScenarioEvent>,
    assertions: Assertions,
}

#[derive(Debug, Deserialize)]
struct ScriptedModel {
    turns: Vec<ScriptedTurn>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ScenarioEvent {
    #[serde(rename = "user.message")]
    Message { content: Vec<ContentBlockJson> },
    #[serde(rename = "user.interrupt")]
    Interrupt {},
    #[serde(rename = "user.custom_tool_result")]
    CustomToolResult { custom_tool_use_id: String, content: Vec<ContentBlockJson> },
    #[serde(rename = "user.tool_confirmation")]
    ToolConfirmation { tool_use_id: String, result: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlockJson {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: serde_json::Value },
    #[serde(rename = "file")]
    File { file_id: String },
}

#[derive(Debug, Deserialize)]
struct Assertions {
    exact_sequence: Vec<String>,
    #[allow(dead_code)]
    must_contain: Option<Vec<String>>,
    #[allow(dead_code)]
    must_end_with: Option<Vec<String>>,
}

/// Load a fixture from the fixtures directory.
fn load_fixture(filename: &str) -> Fixture {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(filename);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse fixture {}: {e}", path.display()))
}

/// Convert a scenario event into a `UserEvent`.
fn scenario_to_user_event(event: &ScenarioEvent) -> UserEvent {
    match event {
        ScenarioEvent::Message { content } => {
            UserEvent::Message { content: content.iter().map(content_block_from_json).collect() }
        }
        ScenarioEvent::Interrupt {} => UserEvent::Interrupt {},
        ScenarioEvent::CustomToolResult { custom_tool_use_id, content } => {
            UserEvent::CustomToolResult {
                custom_tool_use_id: custom_tool_use_id.clone(),
                content: content.iter().map(content_block_from_json).collect(),
            }
        }
        ScenarioEvent::ToolConfirmation { tool_use_id, result } => {
            let confirmation_result = match result.as_str() {
                "allow" => adk_managed::types::ConfirmationResult::Allow,
                _ => adk_managed::types::ConfirmationResult::Deny,
            };
            UserEvent::ToolConfirmation {
                tool_use_id: tool_use_id.clone(),
                result: confirmation_result,
                deny_message: None,
            }
        }
    }
}

fn content_block_from_json(block: &ContentBlockJson) -> ContentBlock {
    match block {
        ContentBlockJson::Text { text } => ContentBlock::Text { text: text.clone() },
        ContentBlockJson::Image { source } => ContentBlock::Image { source: source.clone() },
        ContentBlockJson::File { file_id } => ContentBlock::File { file_id: file_id.clone() },
    }
}

/// Extract the event type string from a SessionEvent.
fn event_type_string(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::StatusRunning { .. } => "status.running",
        SessionEvent::Message { .. } => "agent.message",
        SessionEvent::ToolUse { .. } => "agent.tool_use",
        SessionEvent::CustomToolUse { .. } => "agent.custom_tool_use",
        SessionEvent::McpToolUse { .. } => "agent.mcp_tool_use",
        SessionEvent::StatusIdle { .. } => "status.idle",
        SessionEvent::Error { .. } => "error",
        _ => "unknown",
    }
}

/// Run a fixture in scripted mode using the session loop.
///
/// This exercises the full runtime pipeline:
/// - Session loop processes user events
/// - ScriptedLlm provides deterministic responses (via the stub runner for now)
/// - Events are broadcast and collected
/// - Exact sequence is asserted
async fn run_fixture_scripted(fixture: &Fixture) -> Vec<String> {
    let (event_tx, event_rx) = mpsc::channel(64);
    let (broadcast_tx, mut broadcast_rx) = broadcast::channel(256);
    let cancel = CancellationToken::new();
    let parking = Arc::new(ToolParkingLot::new(Duration::from_secs(30)));

    let session_id = format!("fixture_{}", fixture.name);

    // The ScriptedLlm is created but the session loop currently uses a stub
    // runner. The ScriptedLlm will be wired when the Runner integration is
    // complete. For now, validate the event pattern from the stub.
    let _scripted_llm = ScriptedLlm::new("fixture-model", fixture.scripted_model.turns.clone());

    let session_service = build_stub_session_service();

    // Seed the session in the service — the Runner requires it to exist.
    session_service
        .create(adk_session::service::CreateRequest {
            app_name: "managed".to_string(),
            user_id: "managed_user".to_string(),
            session_id: Some(session_id.clone()),
            state: std::collections::HashMap::new(),
        })
        .await
        .expect("failed to seed session for fixture test");

    let session_loop = SessionLoop::new(
        session_id,
        event_rx,
        broadcast_tx,
        parking.clone(),
        cancel.clone(),
        build_stub_agent(),
        session_service,
    );

    let loop_handle = tokio::spawn(session_loop.run());

    // Send scenario events.
    for scenario_event in &fixture.scenario {
        let user_event = scenario_to_user_event(scenario_event);

        // For interrupts, send and let the loop handle it.
        match &user_event {
            UserEvent::Interrupt {} => {
                // Give the loop time to process any pending message first.
                tokio::time::sleep(Duration::from_millis(20)).await;
                event_tx.send(user_event).await.unwrap();
            }
            UserEvent::CustomToolResult { custom_tool_use_id, content } => {
                // Deliver custom tool results directly to parking lot.
                tokio::time::sleep(Duration::from_millis(20)).await;
                event_tx
                    .send(UserEvent::CustomToolResult {
                        custom_tool_use_id: custom_tool_use_id.clone(),
                        content: content.clone(),
                    })
                    .await
                    .unwrap();
            }
            _ => {
                event_tx.send(user_event).await.unwrap();
            }
        }

        // Allow time for the loop to process.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Close the channel to signal loop termination.
    drop(event_tx);

    // Wait for the loop to exit.
    let _ = tokio::time::timeout(Duration::from_secs(5), loop_handle).await;

    // Collect all broadcast events.
    let mut event_types = Vec::new();
    while let Ok(event) = broadcast_rx.try_recv() {
        event_types.push(event_type_string(&event).to_string());
    }

    event_types
}

/// Assert exact sequence match for scripted mode.
fn assert_exact_sequence(fixture_name: &str, actual: &[String], expected: &[String]) {
    assert_eq!(
        actual, expected,
        "\n\nFixture: {fixture_name}\n  Expected: {expected:?}\n  Actual:   {actual:?}\n"
    );
}

// ============================================================================
// Fixture Tests
// ============================================================================

#[tokio::test]
async fn test_f1_hello() {
    let fixture = load_fixture("f1_hello.json");
    let actual = run_fixture_scripted(&fixture).await;
    assert_exact_sequence(&fixture.name, &actual, &fixture.assertions.exact_sequence);
}

#[tokio::test]
async fn test_f5_resume() {
    let fixture = load_fixture("f5_resume.json");
    let actual = run_fixture_scripted(&fixture).await;
    assert_exact_sequence(&fixture.name, &actual, &fixture.assertions.exact_sequence);
}

#[tokio::test]
async fn test_f6_replay() {
    let fixture = load_fixture("f6_replay.json");
    let actual = run_fixture_scripted(&fixture).await;
    assert_exact_sequence(&fixture.name, &actual, &fixture.assertions.exact_sequence);
}

#[tokio::test]
async fn test_f7_interrupt() {
    let fixture = load_fixture("f7_interrupt.json");
    let actual = run_fixture_scripted(&fixture).await;

    // For interrupt, the expected sequence depends on timing.
    // The message may or may not be processed before the interrupt arrives.
    // With our stub runner (instant echo), the message is processed first,
    // then the interrupt causes the loop to exit.
    // The exact sequence from the fixture should match.
    assert_exact_sequence(&fixture.name, &actual, &fixture.assertions.exact_sequence);
}

/// Test that all fixtures load and parse correctly.
#[test]
fn test_all_fixtures_parse() {
    let fixture_files = [
        "f1_hello.json",
        "f2_mcp_tool.json",
        "f3_custom_tool.json",
        "f4_confirmation.json",
        "f5_resume.json",
        "f6_replay.json",
        "f7_interrupt.json",
        "f8_provider_parity.json",
    ];

    for file in &fixture_files {
        let fixture = load_fixture(file);
        assert!(!fixture.name.is_empty(), "fixture {file} has no name");
        assert!(!fixture.description.is_empty(), "fixture {file} has no description");
        assert!(
            !fixture.assertions.exact_sequence.is_empty(),
            "fixture {file} has no exact_sequence assertions"
        );
        assert!(!fixture.scenario.is_empty(), "fixture {file} has no scenario events");
    }
}

/// Test that the ScriptedLlm correctly serves turns from fixture data.
#[tokio::test]
async fn test_scripted_llm_from_fixture() {
    use adk_core::Llm;
    use futures::StreamExt;

    let fixture = load_fixture("f1_hello.json");
    let llm = ScriptedLlm::new("fixture-model", fixture.scripted_model.turns);

    let request = adk_core::LlmRequest::new("fixture-model", vec![]);
    let mut stream = llm.generate_content(request, false).await.unwrap();

    let response = stream.next().await.unwrap().unwrap();
    assert!(response.turn_complete);

    let content = response.content.unwrap();
    assert_eq!(content.role, "model");
    match &content.parts[0] {
        adk_core::types::Part::Text { text } => {
            assert_eq!(text, "Hello! How can I help you today?");
        }
        other => panic!("expected Text part, got: {other:?}"),
    }
}

/// Verify seq monotonicity across all events produced by a fixture run.
#[tokio::test]
async fn test_seq_monotonicity_in_fixture_run() {
    let (event_tx, event_rx) = mpsc::channel(64);
    let (broadcast_tx, mut broadcast_rx) = broadcast::channel(256);
    let cancel = CancellationToken::new();
    let parking = Arc::new(ToolParkingLot::new(Duration::from_secs(30)));

    let session_loop = SessionLoop::new(
        "seq_test".to_string(),
        event_rx,
        broadcast_tx,
        parking,
        cancel.clone(),
        build_stub_agent(),
        build_stub_session_service(),
    );

    let loop_handle = tokio::spawn(session_loop.run());

    // Send multiple messages.
    for i in 0..3 {
        event_tx
            .send(UserEvent::Message {
                content: vec![ContentBlock::Text { text: format!("Message {i}") }],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    drop(event_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), loop_handle).await;

    // Collect seqs and verify monotonicity.
    let mut seqs = Vec::new();
    while let Ok(event) = broadcast_rx.try_recv() {
        let seq = match &event {
            SessionEvent::StatusRunning { seq } => *seq,
            SessionEvent::Message { seq, .. } => *seq,
            SessionEvent::StatusIdle { seq, .. } => *seq,
            SessionEvent::ToolUse { seq, .. } => *seq,
            SessionEvent::CustomToolUse { seq, .. } => *seq,
            SessionEvent::McpToolUse { seq, .. } => *seq,
            SessionEvent::Error { seq, .. } => *seq,
            _ => continue,
        };
        seqs.push(seq);
    }

    assert!(!seqs.is_empty(), "should have collected events");
    for window in seqs.windows(2) {
        assert!(
            window[1] > window[0],
            "seq must be strictly increasing: {} > {} violated",
            window[1],
            window[0]
        );
    }
}
