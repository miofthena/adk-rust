//! Tests for the optional [`RunObserver`] lifecycle hook (Change 3).

use adk_core::{Agent, Content, Event, EventStream, InvocationContext, Result, SessionId, UserId};
use adk_runner::{RunObserver, Runner, RuntimeEvent, RuntimeEventKind};
use adk_session::{
    CreateRequest, DeleteRequest, Events, GetRequest, ListRequest, Session, SessionService, State,
};
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use std::sync::{Arc, Mutex};

// --- Minimal in-memory session service (mirrors run_lifecycle_test.rs) ---

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

// --- A simple agent that yields one model text event. ---

struct EchoAgent;
#[async_trait]
impl Agent for EchoAgent {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "yields one model event"
    }
    fn sub_agents(&self) -> &[Arc<dyn Agent>] {
        &[]
    }
    async fn run(&self, ctx: Arc<dyn InvocationContext>) -> Result<EventStream> {
        let inv = ctx.invocation_id().to_string();
        let s = stream! {
            let mut event = Event::new(inv.as_str());
            event.author = "echo".to_string();
            event.llm_response.content = Some(Content::new("model").with_text("done"));
            yield Ok(event);
        };
        Ok(Box::pin(s))
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

#[tokio::test]
async fn observer_records_started_and_completed_with_increasing_sequence() {
    let observer = Arc::new(RecordingObserver::default());
    let runner = Runner::builder()
        .app_name("test_app")
        .agent(Arc::new(EchoAgent) as Arc<dyn Agent>)
        .session_service(Arc::new(MockSessionService) as Arc<dyn SessionService>)
        .run_observer(observer.clone() as Arc<dyn RunObserver>)
        .build()
        .unwrap();

    let sid = SessionId::new("sess-A").unwrap();
    let mut stream = runner.run(user(), sid, content()).await.unwrap();

    // Drain fully so the terminal lifecycle event is emitted.
    let mut event_count = 0;
    while let Some(item) = stream.next().await {
        item.unwrap();
        event_count += 1;
    }
    assert!(event_count >= 1, "the agent yielded at least one event");

    let recorded = observer.events.lock().unwrap().clone();
    assert!(!recorded.is_empty(), "observer received events");

    // Ids are correct on every emitted event.
    for ev in &recorded {
        assert_eq!(ev.session_id, "sess-A");
        assert!(ev.invocation_id.starts_with("inv-"));
    }
    let inv = recorded[0].invocation_id.clone();
    assert!(recorded.iter().all(|e| e.invocation_id == inv), "single invocation");

    // Sequence is monotonically increasing across the whole run.
    for pair in recorded.windows(2) {
        assert!(pair[1].sequence > pair[0].sequence, "sequence strictly increases");
    }

    let started = recorded
        .iter()
        .find(|e| e.kind == RuntimeEventKind::InvocationStarted)
        .expect("a started event was emitted");
    let completed = recorded
        .iter()
        .find(|e| e.kind == RuntimeEventKind::InvocationCompleted)
        .expect("a completed event was emitted");

    // started precedes completed.
    assert!(started.sequence < completed.sequence);

    // A clean run emits neither failed nor cancelled.
    assert!(!recorded.iter().any(|e| e.kind == RuntimeEventKind::InvocationFailed));
    assert!(!recorded.iter().any(|e| e.kind == RuntimeEventKind::InvocationCancelled));
}

#[tokio::test]
async fn no_observer_is_zero_overhead_and_runs_normally() {
    // Sanity: the default (no observer) path still works and yields the event.
    let runner = Runner::builder()
        .app_name("test_app")
        .agent(Arc::new(EchoAgent) as Arc<dyn Agent>)
        .session_service(Arc::new(MockSessionService) as Arc<dyn SessionService>)
        .build()
        .unwrap();

    let sid = SessionId::new("sess-B").unwrap();
    let mut stream = runner.run(user(), sid, content()).await.unwrap();
    let mut n = 0;
    while let Some(item) = stream.next().await {
        item.unwrap();
        n += 1;
    }
    assert!(n >= 1);
}
