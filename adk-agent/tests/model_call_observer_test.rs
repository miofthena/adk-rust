//! End-to-end tests for model-call runtime observation.
//!
//! Wires an `LlmAgent` (with a fake streaming model) through the real `Runner`
//! with a recording `RunObserver` and asserts that:
//!   - `ModelCallStarted` is emitted before the provider call and
//!     `ModelCallCompleted` after it, carrying token usage + finish reason,
//!   - their sequence numbers stay strictly monotonic with the runner's own
//!     invocation lifecycle events (they share one per-run counter),
//!   - the no-observer path still runs and yields the reply unchanged.

use adk_agent::LlmAgentBuilder;
use adk_core::{
    Agent, Content, Event, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part,
    Result, SessionId, UsageMetadata, UserId,
};
use adk_runner::{RunObserver, Runner, RuntimeEvent, RuntimeEventKind};
use adk_session::{
    CreateRequest, DeleteRequest, Events, GetRequest, ListRequest, Session, SessionService, State,
};
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use std::sync::{Arc, Mutex};

// --- A fake model: two partial text deltas then a TERMINAL chunk carrying the
// final usage + finish_reason (the common streaming shape). ---

struct StreamingModel;

#[async_trait]
impl Llm for StreamingModel {
    fn name(&self) -> &str {
        "fake-streaming-model"
    }

    async fn generate_content(&self, _req: LlmRequest, stream: bool) -> Result<LlmResponseStream> {
        assert!(stream, "agent requests streaming internally");
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

// --- Minimal in-memory session service (mirrors run_observer_test.rs). ---

struct MockEvents;
impl Events for MockEvents {
    fn all(&self) -> Vec<Event> {
        vec![]
    }
    fn len(&self) -> usize {
        0
    }
    fn at(&self, _index: usize) -> Option<&Event> {
        None
    }
}

struct MockState;
impl adk_session::ReadonlyState for MockState {
    fn get(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
    fn all(&self) -> std::collections::HashMap<String, serde_json::Value> {
        std::collections::HashMap::new()
    }
}
impl State for MockState {
    fn get(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
    fn set(&mut self, _key: String, _value: serde_json::Value) {}
    fn all(&self) -> std::collections::HashMap<String, serde_json::Value> {
        std::collections::HashMap::new()
    }
}

struct MockSession {
    id: String,
    app_name: String,
    user_id: String,
    events: MockEvents,
    state: MockState,
}
impl Session for MockSession {
    fn id(&self) -> &str {
        &self.id
    }
    fn app_name(&self) -> &str {
        &self.app_name
    }
    fn user_id(&self) -> &str {
        &self.user_id
    }
    fn state(&self) -> &dyn State {
        &self.state
    }
    fn events(&self) -> &dyn Events {
        &self.events
    }
    fn last_update_time(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

struct MockSessionService;
#[async_trait]
impl SessionService for MockSessionService {
    async fn create(&self, _req: CreateRequest) -> Result<Box<dyn Session>> {
        unimplemented!()
    }
    async fn get(&self, req: GetRequest) -> Result<Box<dyn Session>> {
        Ok(Box::new(MockSession {
            id: req.session_id,
            app_name: req.app_name,
            user_id: req.user_id,
            events: MockEvents,
            state: MockState,
        }))
    }
    async fn list(&self, _req: ListRequest) -> Result<Vec<Box<dyn Session>>> {
        Ok(vec![])
    }
    async fn delete(&self, _req: DeleteRequest) -> Result<()> {
        Ok(())
    }
    async fn append_event(&self, _session_id: &str, _event: Event) -> Result<()> {
        Ok(())
    }
}

// --- Recording observer. ---

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<RuntimeEvent>>,
}
#[async_trait]
impl RunObserver for RecordingObserver {
    async fn on_event(&self, event: RuntimeEvent) -> Result<()> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

fn user() -> UserId {
    UserId::new("user-1").unwrap()
}
fn content() -> Content {
    Content::new("user").with_text("hi")
}

fn build_agent() -> Arc<dyn Agent> {
    Arc::new(
        LlmAgentBuilder::new("echo-llm")
            .model(Arc::new(StreamingModel) as Arc<dyn Llm>)
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn model_call_started_then_completed_ordered_with_lifecycle() {
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::builder()
        .app_name("test_app")
        .agent(build_agent())
        .session_service(Arc::new(MockSessionService) as Arc<dyn SessionService>)
        .run_observer(observer.clone() as Arc<dyn RunObserver>)
        .build()
        .unwrap();

    let sid = SessionId::new("sess-M").unwrap();
    let mut stream = runner.run(user(), sid, content()).await.unwrap();
    let mut reply = String::new();
    while let Some(item) = stream.next().await {
        let ev = item.unwrap();
        if let Some(c) = &ev.llm_response.content
            && !ev.llm_response.partial
        {
            for p in &c.parts {
                if let Part::Text { text } = p {
                    reply.push_str(text);
                }
            }
        }
    }
    assert_eq!(reply, "Hello world", "agent produced the accumulated reply");

    let recorded = observer.events.lock().unwrap().clone();
    assert!(!recorded.is_empty(), "observer received events");

    // Sequence numbers are strictly monotonic across the WHOLE run (model-call
    // events share the runner's per-run counter).
    for pair in recorded.windows(2) {
        assert!(
            pair[1].sequence > pair[0].sequence,
            "sequence strictly increases: {:?} !> {:?}",
            pair[1],
            pair[0]
        );
    }

    let find = |k: RuntimeEventKind| recorded.iter().find(|e| e.kind == k);

    let started = find(RuntimeEventKind::ModelCallStarted).expect("ModelCallStarted emitted");
    let completed = find(RuntimeEventKind::ModelCallCompleted).expect("ModelCallCompleted emitted");
    let queued = find(RuntimeEventKind::InvocationQueued).expect("InvocationQueued emitted");
    let inv_completed =
        find(RuntimeEventKind::InvocationCompleted).expect("InvocationCompleted emitted");

    // Exactly one model call start/complete for this single-call run.
    assert_eq!(
        recorded.iter().filter(|e| e.kind == RuntimeEventKind::ModelCallStarted).count(),
        1,
        "exactly one ModelCallStarted"
    );
    assert_eq!(
        recorded.iter().filter(|e| e.kind == RuntimeEventKind::ModelCallCompleted).count(),
        1,
        "exactly one ModelCallCompleted"
    );

    // Ordering relative to invocation lifecycle: queued precedes the call, the
    // call starts before it completes, and completion precedes invocation end.
    assert!(queued.sequence < started.sequence, "queued before model start");
    assert!(started.sequence < completed.sequence, "model start before model complete");
    assert!(completed.sequence < inv_completed.sequence, "model complete before invocation end");

    // Bounded, PII-free metadata: model id, finish reason as a string, and the
    // token counts (no prompt/response bodies).
    assert_eq!(started.metadata.get("model").map(String::as_str), Some("fake-streaming-model"));
    assert_eq!(completed.metadata.get("model").map(String::as_str), Some("fake-streaming-model"));
    assert_eq!(completed.metadata.get("finish_reason").map(String::as_str), Some("Stop"));
    assert_eq!(completed.metadata.get("total_tokens").map(String::as_str), Some("18"));
    assert_eq!(completed.metadata.get("prompt_tokens").map(String::as_str), Some("11"));
    assert_eq!(completed.metadata.get("completion_tokens").map(String::as_str), Some("7"));
    // No prompt/response text leaks into any metadata value.
    for ev in &recorded {
        for v in ev.metadata.values() {
            assert!(!v.contains("Hello"), "no response text in metadata");
            assert!(!v.contains("hi"), "no prompt text in metadata");
        }
    }

    // Model-call events carry the run's identifiers.
    for ev in [started, completed] {
        assert_eq!(ev.session_id, "sess-M");
        assert!(ev.invocation_id.starts_with("inv-"));
        assert_eq!(ev.agent_name, "echo-llm");
    }

    // A clean run: no failure/cancellation.
    assert!(!recorded.iter().any(|e| e.kind == RuntimeEventKind::InvocationFailed));
    assert!(!recorded.iter().any(|e| e.kind == RuntimeEventKind::InvocationCancelled));
}

#[tokio::test]
async fn no_observer_path_runs_and_emits_nothing() {
    // Without an observer the LlmAgent path is unchanged: it runs, yields the
    // reply, and (by construction) records no runtime events.
    let runner = Runner::builder()
        .app_name("test_app")
        .agent(build_agent())
        .session_service(Arc::new(MockSessionService) as Arc<dyn SessionService>)
        .build()
        .unwrap();

    let sid = SessionId::new("sess-N").unwrap();
    let mut stream = runner.run(user(), sid, content()).await.unwrap();
    let mut reply = String::new();
    let mut n = 0;
    while let Some(item) = stream.next().await {
        let ev = item.unwrap();
        n += 1;
        if let Some(c) = &ev.llm_response.content
            && !ev.llm_response.partial
        {
            for p in &c.parts {
                if let Part::Text { text } = p {
                    reply.push_str(text);
                }
            }
        }
    }
    assert!(n >= 1, "the agent yielded at least one event");
    assert_eq!(reply, "Hello world", "reply unchanged on the no-observer path");
}
