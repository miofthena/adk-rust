//! Tests for invocation-scoped run lifecycle + per-session serialization
//! (Release C: `Runner::start` / `RunHandle`, the invocation registry, and
//! `SessionConcurrencyPolicy`).

use adk_core::{Agent, Content, Event, EventStream, InvocationContext, Result, SessionId, UserId};
use adk_runner::{Runner, SessionConcurrencyPolicy};
use adk_session::{
    CreateRequest, DeleteRequest, Events, GetRequest, ListRequest, Session, SessionService, State,
};
use async_stream::stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

// ---------------------------------------------------------------------------
// Minimal in-memory session service (mirrors runner_tests.rs mocks)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// A controllable agent: records the invocation ids that begin running (via a
// shared log + a `started` semaphore the test can wait on), then blocks until
// either the `release` semaphore hands it a permit OR its cancellation token
// fires. This lets tests observe gate ordering deterministically without sleeps.
// ---------------------------------------------------------------------------

struct GateAgent {
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Agent for GateAgent {
    fn name(&self) -> &str {
        "gate"
    }
    fn description(&self) -> &str {
        "controllable gate agent"
    }
    fn sub_agents(&self) -> &[Arc<dyn Agent>] {
        &[]
    }
    async fn run(&self, ctx: Arc<dyn InvocationContext>) -> Result<EventStream> {
        let inv = ctx.invocation_id().to_string();
        // Executed synchronously when the runner calls `agent.run()` — i.e. once
        // this run has passed registration and (under Serialize) acquired the gate.
        self.log.lock().unwrap().push(inv.clone());
        self.started.add_permits(1);
        let release = self.release.clone();
        let token = ctx.cancellation_token();
        let s = stream! {
            tokio::select! {
                _ = async { if let Ok(p) = release.acquire().await { p.forget(); } } => {}
                _ = async {
                    match token {
                        Some(t) => t.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                } => {}
            }
            let mut event = Event::new(inv.as_str());
            event.author = "gate".to_string();
            event.llm_response.content = Some(Content::new("model").with_text("done"));
            yield Ok(event);
        };
        Ok(Box::pin(s))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn user() -> UserId {
    UserId::new("user-1").unwrap()
}

fn content() -> Content {
    Content::new("user").with_text("hi")
}

fn build_runner(policy: SessionConcurrencyPolicy, agent: Arc<GateAgent>) -> Runner {
    Runner::builder()
        .app_name("test_app")
        .agent(agent as Arc<dyn Agent>)
        .session_service(Arc::new(MockSessionService) as Arc<dyn SessionService>)
        .session_concurrency(policy)
        .build()
        .unwrap()
}

/// Handles for driving a [`GateAgent`] from a test: the agent itself plus the
/// `started`/`release` semaphores and the ordered start log it shares.
struct Fixture {
    agent: Arc<GateAgent>,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    log: Arc<Mutex<Vec<String>>>,
}

fn new_agent() -> Fixture {
    let started = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let agent = Arc::new(GateAgent {
        started: started.clone(),
        release: release.clone(),
        log: log.clone(),
    });
    Fixture { agent, started, release, log }
}

/// Spawn a task that drains a stream to completion, returning the event count.
fn spawn_drain(mut events: EventStream) -> tokio::task::JoinHandle<usize> {
    tokio::spawn(async move {
        let mut n = 0usize;
        while events.next().await.is_some() {
            n += 1;
        }
        n
    })
}

/// Wait until `started` has accumulated `n` permits (i.e. `n` agents began
/// running), consuming them. Bounded so a bug fails fast instead of hanging.
async fn wait_started(started: &Semaphore, n: u32) {
    let permit = tokio::time::timeout(Duration::from_secs(5), started.acquire_many(n))
        .await
        .expect("timed out waiting for agents to start")
        .unwrap();
    permit.forget();
}

// ---------------------------------------------------------------------------
// (1) Two concurrent start()s on the SAME session register DISTINCT tokens.
// (2) interrupt_invocation(A) cancels A but not B.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_same_session_distinct_tokens_and_targeted_interrupt() {
    let Fixture { agent, started, log, .. } = new_agent();
    let runner = build_runner(SessionConcurrencyPolicy::AllowConcurrent, agent);
    let sid = SessionId::new("sess-A").unwrap();

    let a = runner.start(user(), sid.clone(), content()).unwrap();
    let b = runner.start(user(), sid.clone(), content()).unwrap();
    let inv_a = a.invocation_id().to_string();
    let inv_b = b.invocation_id().to_string();

    // Distinct invocation ids (no overwrite of the shared session slot).
    assert_ne!(inv_a, inv_b);

    let da = spawn_drain(a.events);
    let db = spawn_drain(b.events);

    // Both run concurrently under AllowConcurrent → both agents start.
    wait_started(&started, 2).await;
    assert_eq!(log.lock().unwrap().len(), 2);

    // Both invocations are registered → targeted interrupt finds each one.
    // (Under the old single-token-per-session scheme, one would have been lost.)
    assert!(runner.interrupt_invocation(&inv_a), "A must be registered");
    // A cancelled → its agent unblocks and emits one event, then completes.
    assert_eq!(da.await.unwrap(), 1);

    // B is untouched by interrupting A: still blocked until we cancel it too.
    assert!(runner.interrupt_invocation(&inv_b), "B still registered after A cancelled");
    assert_eq!(db.await.unwrap(), 1);
}

// ---------------------------------------------------------------------------
// (3) interrupt(session) cancels ALL invocations of the session.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interrupt_session_cancels_all() {
    let Fixture { agent, started, .. } = new_agent();
    let runner = build_runner(SessionConcurrencyPolicy::AllowConcurrent, agent);
    let sid = SessionId::new("sess-B").unwrap();

    let a = runner.start(user(), sid.clone(), content()).unwrap();
    let b = runner.start(user(), sid.clone(), content()).unwrap();
    let da = spawn_drain(a.events);
    let db = spawn_drain(b.events);

    wait_started(&started, 2).await;

    assert!(runner.interrupt(sid.as_str()), "at least one invocation cancelled");

    // Both complete (each emits one event on cancellation).
    assert_eq!(da.await.unwrap(), 1);
    assert_eq!(db.await.unwrap(), 1);

    // Session index is now empty.
    assert!(runner.active_session_ids().is_empty());
    assert!(!runner.interrupt(sid.as_str()), "no active run remains");
}

// ---------------------------------------------------------------------------
// (4) Repeated cancel is idempotent.
// (5) Registry is EMPTY after a run completes/drops.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repeated_cancel_idempotent_and_registry_cleaned() {
    let Fixture { agent, .. } = new_agent();
    let runner = build_runner(SessionConcurrencyPolicy::AllowConcurrent, agent);
    let sid = SessionId::new("sess-C").unwrap();

    let handle = runner.start(user(), sid.clone(), content()).unwrap();
    let inv = handle.invocation_id().to_string();
    // Idempotent: cancelling the handle repeatedly must not panic.
    handle.cancel();
    handle.cancel();
    handle.cancel();

    // Drain to completion — the run was cancelled before its stream was polled.
    let mut events = handle.events;
    while events.next().await.is_some() {}
    drop(events);

    // Registry fully drained by the in-stream RAII cleanup.
    assert!(runner.active_session_ids().is_empty(), "registry must be empty after completion");
    assert!(!runner.interrupt(sid.as_str()));
    assert!(!runner.interrupt_invocation(&inv), "completed run must be deregistered");
}

// ---------------------------------------------------------------------------
// (5b) Registry empty after a NORMAL (released) completion too.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_empty_after_normal_completion() {
    let Fixture { agent, started, release, .. } = new_agent();
    let runner = build_runner(SessionConcurrencyPolicy::AllowConcurrent, agent);
    let sid = SessionId::new("sess-D").unwrap();

    let handle = runner.start(user(), sid.clone(), content()).unwrap();
    let drain = spawn_drain(handle.events);

    wait_started(&started, 1).await;
    assert_eq!(runner.active_session_ids(), vec![sid.as_str().to_string()]);

    release.add_permits(1); // let the agent finish normally
    assert_eq!(drain.await.unwrap(), 1);

    assert!(runner.active_session_ids().is_empty());
}

// ---------------------------------------------------------------------------
// (6) Serialize: two runs on ONE session are sequential; two DIFFERENT sessions
//     overlap.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn serialize_same_session_sequential() {
    let Fixture { agent, started, release, log } = new_agent();
    let runner = build_runner(
        SessionConcurrencyPolicy::Serialize { max_queue: 8, retained_sessions: 16 },
        agent,
    );
    let sid = SessionId::new("sess-S").unwrap();

    let a = runner.start(user(), sid.clone(), content()).unwrap();
    let b = runner.start(user(), sid.clone(), content()).unwrap();
    let da = spawn_drain(a.events);
    let db = spawn_drain(b.events);

    // Exactly ONE agent runs at a time: whichever acquires the gate first.
    wait_started(&started, 1).await;
    assert_eq!(log.lock().unwrap().len(), 1, "second run must be queued behind the gate");

    // Release the holder → it completes → permit frees → the queued run acquires.
    release.add_permits(1);
    wait_started(&started, 1).await;
    assert_eq!(log.lock().unwrap().len(), 2);

    release.add_permits(1);
    assert_eq!(da.await.unwrap(), 1);
    assert_eq!(db.await.unwrap(), 1);

    // The two runs used distinct invocation ids.
    let ids = log.lock().unwrap().clone();
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn serialize_different_sessions_overlap() {
    let Fixture { agent, started, release, log } = new_agent();
    let runner = build_runner(
        SessionConcurrencyPolicy::Serialize { max_queue: 8, retained_sessions: 16 },
        agent,
    );

    let a = runner.start(user(), SessionId::new("sess-X").unwrap(), content()).unwrap();
    let b = runner.start(user(), SessionId::new("sess-Y").unwrap(), content()).unwrap();
    let da = spawn_drain(a.events);
    let db = spawn_drain(b.events);

    // Different sessions → different gates → both run concurrently.
    // (If serialization wrongly spanned sessions, this would hang and time out.)
    wait_started(&started, 2).await;
    assert_eq!(log.lock().unwrap().len(), 2);

    release.add_permits(2);
    assert_eq!(da.await.unwrap(), 1);
    assert_eq!(db.await.unwrap(), 1);
}

// ---------------------------------------------------------------------------
// (7) Serialize { max_queue: 0 } / over-cap yields the structured busy error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn serialize_max_queue_zero_rejects_with_busy_error() {
    let Fixture { agent, started, release, .. } = new_agent();
    let runner = build_runner(
        SessionConcurrencyPolicy::Serialize { max_queue: 0, retained_sessions: 16 },
        agent,
    );
    let sid = SessionId::new("sess-Q").unwrap();

    // A acquires and holds the gate.
    let a = runner.start(user(), sid.clone(), content()).unwrap();
    let da = spawn_drain(a.events);
    wait_started(&started, 1).await;

    // B would have to queue (max_queue == 0) → rejected with the busy error.
    let b = runner.start(user(), sid.clone(), content()).unwrap();
    let mut events_b = b.events;
    let first = events_b.next().await.expect("expected a busy error item");
    let err = first.expect_err("must be the structured busy error");
    assert_eq!(err.code, "runner.session_busy");
    assert!(events_b.next().await.is_none(), "rejected run yields nothing further");

    // The rejected run did NOT run the agent and left the registry consistent.
    release.add_permits(1);
    assert_eq!(da.await.unwrap(), 1);
    assert!(runner.active_session_ids().is_empty());
}

// ---------------------------------------------------------------------------
// (8) A queued run is cancellable BEFORE it acquires the gate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queued_run_cancellable_before_acquire() {
    let Fixture { agent, started, release, log } = new_agent();
    let runner = build_runner(
        SessionConcurrencyPolicy::Serialize { max_queue: 8, retained_sessions: 16 },
        agent,
    );
    let sid = SessionId::new("sess-W").unwrap();

    // A acquires and holds the gate.
    let a = runner.start(user(), sid.clone(), content()).unwrap();
    let da = spawn_drain(a.events);
    wait_started(&started, 1).await;
    assert_eq!(log.lock().unwrap().len(), 1);

    // B enters, registers, and blocks queued at the gate (does NOT start the agent).
    let b = runner.start(user(), sid.clone(), content()).unwrap();
    let inv_b = b.invocation_id().to_string();
    let mut events_b = b.events;
    // Drive B forward: it should be Pending (queued at the gate), producing nothing.
    let pending = tokio::time::timeout(Duration::from_millis(150), events_b.next()).await;
    assert!(pending.is_err(), "B must be queued (Pending), not producing an event");
    assert_eq!(log.lock().unwrap().len(), 1, "queued run must not start its agent");

    // Cancel B specifically while it is still queued.
    assert!(runner.interrupt_invocation(&inv_b), "B is registered while queued");

    // B's queued acquire is cancelled → the run returns WITHOUT running the agent.
    let drained_b = tokio::time::timeout(Duration::from_secs(5), async {
        while events_b.next().await.is_some() {}
    })
    .await;
    assert!(drained_b.is_ok(), "cancelled queued run must complete");
    assert_eq!(log.lock().unwrap().len(), 1, "cancelled queued run never ran its agent");

    // A is unaffected: release it and confirm it completed.
    release.add_permits(1);
    assert_eq!(da.await.unwrap(), 1);
    assert!(runner.active_session_ids().is_empty());
}
