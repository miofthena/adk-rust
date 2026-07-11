//! Tests for the streaming terminal-accumulation fix (Change 1) and the
//! `ModelRequestPersistence` policy (Change 2).
//!
//! Change 1: in SSE streaming mode the terminal (`partial == false`) event —
//! the one the Runner persists — must carry the FULL accumulated reply plus the
//! final usage/finish_reason, even when the provider's last chunk has
//! `content: null` (the common case).
//!
//! Change 2: under `ModelRequestPersistence::Metadata` the persisted event
//! carries a compact digest (no prompt text / image bytes); under `Full` it
//! carries the whole request.

use adk_agent::LlmAgentBuilder;
use adk_core::{
    Agent, Content, FinishReason, InvocationContext, Llm, LlmRequest, LlmResponse,
    LlmResponseStream, ModelRequestPersistence, Part, Result, RunConfig, Session, State,
    UsageMetadata,
};
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// A model that streams two partial text deltas and then a TERMINAL chunk whose
// `content` is `None` — exactly the shape a real streaming provider emits (the
// text lives in the dropped partials; the terminal carries only usage +
// finish_reason). This is the case the old code persisted as `content: null`.
struct NullTerminalModel;

#[async_trait]
impl Llm for NullTerminalModel {
    fn name(&self) -> &str {
        "null-terminal-model"
    }

    async fn generate_content(&self, _req: LlmRequest, stream: bool) -> Result<LlmResponseStream> {
        assert!(stream, "agent should request streaming internally");
        let s = stream! {
            for delta in ["Hello", " world"] {
                yield Ok(LlmResponse {
                    content: Some(Content::new("model").with_text(delta)),
                    usage_metadata: None,
                    finish_reason: None,
                    citation_metadata: None,
                    partial: true,
                    turn_complete: false,
                    interrupted: false,
                    error_code: None,
                    error_message: None,
                    provider_metadata: None,
                    interaction_id: None,
                });
            }
            // Terminal chunk: NO content, but final usage + finish_reason.
            yield Ok(LlmResponse {
                content: None,
                usage_metadata: Some(UsageMetadata {
                    prompt_token_count: 11,
                    candidates_token_count: 7,
                    total_token_count: 18,
                    ..Default::default()
                }),
                finish_reason: Some(FinishReason::Stop),
                citation_metadata: None,
                partial: false,
                turn_complete: true,
                interrupted: false,
                error_code: None,
                error_message: None,
                provider_metadata: None,
                interaction_id: None,
            });
        };
        Ok(Box::pin(s))
    }
}

// --- Minimal invocation context with a configurable RunConfig ---

struct MockSession;
impl Session for MockSession {
    fn id(&self) -> &str {
        "session-1"
    }
    fn app_name(&self) -> &str {
        "test-app"
    }
    fn user_id(&self) -> &str {
        "user-1"
    }
    fn state(&self) -> &dyn State {
        &MockState
    }
    fn conversation_history(&self) -> Vec<Content> {
        Vec::new()
    }
}

struct MockState;
impl State for MockState {
    fn get(&self, _key: &str) -> Option<Value> {
        None
    }
    fn set(&mut self, _key: String, _value: Value) {}
    fn all(&self) -> HashMap<String, Value> {
        HashMap::new()
    }
}

struct MockContext {
    session: MockSession,
    user_content: Content,
    run_config: RunConfig,
}

impl MockContext {
    fn new(prompt: &str, run_config: RunConfig) -> Self {
        Self {
            session: MockSession,
            user_content: Content::new("user").with_text(prompt),
            run_config,
        }
    }
}

#[async_trait]
impl adk_core::ReadonlyContext for MockContext {
    fn invocation_id(&self) -> &str {
        "inv-1"
    }
    fn agent_name(&self) -> &str {
        "test-agent"
    }
    fn user_id(&self) -> &str {
        "user-1"
    }
    fn app_name(&self) -> &str {
        "test-app"
    }
    fn session_id(&self) -> &str {
        "session-1"
    }
    fn branch(&self) -> &str {
        "main"
    }
    fn user_content(&self) -> &Content {
        &self.user_content
    }
}

#[async_trait]
impl adk_core::CallbackContext for MockContext {
    fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
        None
    }
}

#[async_trait]
impl InvocationContext for MockContext {
    fn agent(&self) -> Arc<dyn Agent> {
        unimplemented!()
    }
    fn memory(&self) -> Option<Arc<dyn adk_core::Memory>> {
        None
    }
    fn session(&self) -> &dyn Session {
        &self.session
    }
    fn run_config(&self) -> &RunConfig {
        &self.run_config
    }
    fn end_invocation(&self) {}
    fn ended(&self) -> bool {
        false
    }
}

/// Drain the agent stream, returning (partial deltas concatenated, terminal event).
async fn run_agent(model: Arc<dyn Llm>, ctx: Arc<MockContext>) -> (String, adk_core::Event) {
    let agent = LlmAgentBuilder::new("test-agent").model(model).build().unwrap();
    let mut stream = agent.run(ctx).await.unwrap();

    let mut partial_text = String::new();
    let mut terminal: Option<adk_core::Event> = None;
    while let Some(result) = stream.next().await {
        let event = result.unwrap();
        let text: String = event
            .llm_response
            .content
            .as_ref()
            .map(|c| {
                c.parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if event.llm_response.partial {
            partial_text.push_str(&text);
        } else if event.llm_response.content.is_some() || event.llm_response.finish_reason.is_some()
        {
            // The non-partial model event of record (ignore any bare state events).
            terminal = Some(event);
        }
    }
    (partial_text, terminal.expect("a terminal event was emitted"))
}

// ---------------------------------------------------------------------------
// Change 1: terminal event carries full accumulated content + usage + finish.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_event_carries_full_reply_not_null() {
    let ctx = Arc::new(MockContext::new("hi", RunConfig::default()));
    let (partial_text, terminal) = run_agent(Arc::new(NullTerminalModel), ctx).await;

    // Client still saw the streamed deltas.
    assert_eq!(partial_text, "Hello world");

    // The persisted terminal is NOT partial and carries the FULL reply (the old
    // bug stored the null last-chunk content here).
    assert!(!terminal.llm_response.partial);
    let text: String = terminal
        .llm_response
        .content
        .as_ref()
        .expect("terminal must carry accumulated content, not null")
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello world");

    // Final usage + finish_reason are taken from the terminal chunk metadata.
    assert_eq!(terminal.llm_response.finish_reason, Some(FinishReason::Stop));
    let usage = terminal.llm_response.usage_metadata.expect("usage present");
    assert_eq!(usage.total_token_count, 18);
}

// ---------------------------------------------------------------------------
// Change 2: Metadata policy stores a digest (no prompt); Full stores the request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn metadata_policy_stores_digest_without_prompt() {
    const SECRET: &str = "TOP_SECRET_PROMPT_TEXT";
    let cfg =
        RunConfig::builder().model_request_persistence(ModelRequestPersistence::Metadata).build();
    let ctx = Arc::new(MockContext::new(SECRET, cfg));
    let (_partial, terminal) = run_agent(Arc::new(NullTerminalModel), ctx).await;

    let req = terminal.llm_request.expect("digest attached under Metadata");
    // The digest carries fingerprints, not the prompt text or image bytes.
    assert!(!req.contains(SECRET), "digest must NOT contain the prompt text");
    let digest: Value = serde_json::from_str(&req).expect("digest is JSON");
    assert_eq!(digest["digest"], Value::Bool(true));
    assert!(digest.get("request_bytes").is_some());
    assert!(digest.get("context_digest").is_some());
    assert!(digest.get("tools_digest").is_some());
    // Usage + finish_reason are folded into the digest.
    assert_eq!(digest["finish_reason"], serde_json::json!("Stop"));

    // The full-request provider-metadata mirror is NOT set under Metadata.
    assert!(!terminal.provider_metadata.contains_key("gcp.vertex.agent.llm_request"));
}

#[tokio::test]
async fn full_policy_stores_whole_request() {
    const SECRET: &str = "TOP_SECRET_PROMPT_TEXT";
    // Default policy is Full.
    let ctx = Arc::new(MockContext::new(SECRET, RunConfig::default()));
    let (_partial, terminal) = run_agent(Arc::new(NullTerminalModel), ctx).await;

    let req = terminal.llm_request.expect("full request attached under Full");
    assert!(req.contains(SECRET), "Full policy must store the whole request");
    // And it is mirrored into provider metadata (historical behavior).
    assert!(terminal.provider_metadata.contains_key("gcp.vertex.agent.llm_request"));
}
