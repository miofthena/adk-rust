use crate::InvocationContext;
use crate::cache::CacheManager;
#[cfg(feature = "artifacts")]
use adk_artifact::ArtifactService;
use adk_core::{
    Agent, AppName, CacheCapable, Content, ContextCacheConfig, EventStream, Memory,
    ReadonlyContext, Result, RunConfig, SessionId, UserId,
};
#[cfg(feature = "plugins")]
use adk_plugin::PluginManager;
use adk_session::SessionService;
#[cfg(feature = "skills")]
use adk_skill::{SkillInjector, SkillInjectorConfig};
use async_stream::stream;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// A handle to a started agent run, returned by [`Runner::start`].
///
/// Carries the invocation's event stream plus the invocation-scoped cancellation
/// handle. Dropping the handle does **not** clean up the run's registry entry or
/// release any concurrency permit — that lifecycle is anchored *inside* the
/// [`EventStream`] itself (RAII), so the stream owns the full lifecycle even when
/// the handle is discarded immediately (as the [`Runner::run`] shim does).
pub struct RunHandle {
    /// The invocation identifier minted for this run (`"inv-<uuid>"`).
    pub invocation_id: String,
    /// The session this run belongs to.
    pub session_id: SessionId,
    /// The event stream produced by the run.
    pub events: EventStream,
    /// The invocation-scoped cancellation token. Cancelling it stops this run.
    cancellation: CancellationToken,
}

impl RunHandle {
    /// Cancel this specific invocation. Idempotent; cancels the child token this
    /// handle owns (no registry lookup).
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Returns the invocation identifier for this run.
    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    /// A clone of this invocation's cancellation token, so a caller can consume
    /// [`RunHandle::events`] while retaining the ability to cancel the run (e.g. an
    /// on-disconnect guard). Cancelling the returned token is equivalent to [`RunHandle::cancel`].
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

/// Policy governing how concurrent runs targeting the **same** session are handled.
///
/// The default is [`AllowConcurrent`](Self::AllowConcurrent), which preserves the
/// historical behavior (no gating — concurrent same-session runs execute in parallel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionConcurrencyPolicy {
    /// Serialize runs per session behind a fair (FIFO) async semaphore.
    ///
    /// `max_queue` bounds how many runs may **wait** behind the in-flight run for a
    /// given session; a run that would exceed it is rejected with a structured busy
    /// error instead of queueing. `retained_sessions` bounds the number of
    /// per-session gates retained in memory (idle gates are evicted LRU).
    Serialize {
        /// Maximum queued (waiting) runs per session before rejection.
        max_queue: usize,
        /// Maximum number of per-session gates retained (LRU-evicted when idle).
        retained_sessions: usize,
    },
    /// Reject a new run if the session already has an in-flight run.
    RejectConcurrent,
    /// Allow unbounded concurrent runs per session (historical default).
    #[default]
    AllowConcurrent,
}

/// Registry of in-flight invocations, keyed by invocation id, with a secondary
/// index from session id to its (FIFO-ordered) invocation ids. One lock guards
/// both maps so they stay consistent.
#[derive(Default)]
struct Registry {
    active: HashMap<String, CancellationToken>,
    by_session: HashMap<String, Vec<String>>,
}

/// A per-session serialization gate: a fair permit-of-1 semaphore plus a depth
/// counter (holder + waiters) used to enforce `max_queue`.
#[derive(Clone)]
struct SessionGate {
    sem: Arc<Semaphore>,
    depth: Arc<AtomicUsize>,
}

/// RAII guard that decrements a session gate's depth counter when a run leaves the
/// gate region (finished after acquiring, or cancelled while queued). The reject
/// path decrements manually before this guard is ever created.
struct DepthGuard {
    depth: Arc<AtomicUsize>,
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bounded map of per-session gates with LRU eviction of idle entries.
struct SessionGates {
    map: HashMap<String, SessionGate>,
    order: Vec<String>,
    capacity: usize,
}

impl SessionGates {
    fn new(capacity: usize) -> Self {
        Self { map: HashMap::new(), order: Vec::new(), capacity: capacity.max(1) }
    }

    /// Get the gate for `session_id`, creating it if absent. Evicts an idle
    /// (unreferenced) LRU gate first when at capacity. The returned gate is a
    /// cheap clone (two `Arc`s) whose existence keeps the entry non-evictable
    /// until the caller drops it.
    fn get_or_create(&mut self, session_id: &str) -> SessionGate {
        if let Some(gate) = self.map.get(session_id) {
            let gate = gate.clone();
            self.touch(session_id);
            return gate;
        }
        if self.map.len() >= self.capacity {
            self.evict_idle();
        }
        let gate =
            SessionGate { sem: Arc::new(Semaphore::new(1)), depth: Arc::new(AtomicUsize::new(0)) };
        self.map.insert(session_id.to_string(), gate.clone());
        self.order.push(session_id.to_string());
        gate
    }

    fn touch(&mut self, session_id: &str) {
        if let Some(pos) = self.order.iter().position(|s| s == session_id) {
            let s = self.order.remove(pos);
            self.order.push(s);
        }
    }

    /// Remove the least-recently-used gate that is currently idle — i.e. not
    /// referenced by any in-flight or queued run. `Arc::strong_count == 1` means
    /// only this map holds the semaphore, so no run is holding or waiting on it,
    /// making eviction safe (it can never split serialization of a live session).
    fn evict_idle(&mut self) {
        if let Some(pos) = self
            .order
            .iter()
            .position(|sid| self.map.get(sid).is_some_and(|g| Arc::strong_count(&g.sem) == 1))
        {
            let sid = self.order.remove(pos);
            self.map.remove(&sid);
        }
    }
}

/// Stable structured error yielded when a run is refused by the session
/// concurrency policy (`RejectConcurrent`, or `Serialize` `max_queue` exceeded).
/// Downstream code detects it by the stable code `"runner.session_busy"`.
fn session_busy_error() -> adk_core::AdkError {
    adk_core::AdkError::new(
        adk_core::ErrorComponent::Server,
        adk_core::ErrorCategory::Unavailable,
        "runner.session_busy",
        "session busy: run rejected by session concurrency policy",
    )
}

/// Configuration for constructing a [`Runner`].
///
/// Use [`Runner::builder()`] for a compile-time-safe way to construct this.
pub struct RunnerConfig {
    /// Application name used for session scoping.
    pub app_name: String,
    /// The root agent to execute.
    pub agent: Arc<dyn Agent>,
    /// Session persistence backend.
    pub session_service: Arc<dyn SessionService>,
    #[cfg(feature = "artifacts")]
    /// Optional artifact storage service.
    pub artifact_service: Option<Arc<dyn ArtifactService>>,
    /// Optional memory/RAG service.
    pub memory_service: Option<Arc<dyn Memory>>,
    #[cfg(feature = "plugins")]
    /// Optional plugin manager for lifecycle hooks.
    pub plugin_manager: Option<Arc<PluginManager>>,
    /// Optional run configuration (streaming mode, etc.)
    /// If not provided, uses default (SSE streaming)
    #[allow(dead_code)]
    pub run_config: Option<RunConfig>,
    /// Optional context compaction configuration.
    /// When set, the runner will periodically summarize older events
    /// to reduce context size sent to the LLM.
    pub compaction_config: Option<adk_core::EventsCompactionConfig>,
    /// Optional context cache configuration for automatic prompt caching lifecycle.
    /// When set alongside `cache_capable`, the runner will automatically create and
    /// manage cached content resources for supported providers.
    ///
    /// When `cache_capable` is set but this field is `None`, the runner
    /// automatically uses [`ContextCacheConfig::default()`] (4096 min tokens,
    /// 600s TTL, refresh every 3 invocations).
    pub context_cache_config: Option<ContextCacheConfig>,
    /// Optional cache-capable model reference for automatic cache management.
    /// Set this to the same model used by the agent if it supports caching.
    pub cache_capable: Option<Arc<dyn CacheCapable>>,
    /// Optional request context from the server's auth middleware bridge.
    /// When set, the runner passes it to `InvocationContext` so that
    /// `user_scopes()` and `user_id()` reflect the authenticated identity.
    pub request_context: Option<adk_core::RequestContext>,
    /// Optional cooperative cancellation token for externally managed runs.
    pub cancellation_token: Option<CancellationToken>,
    /// Optional session concurrency policy governing same-session runs.
    /// Defaults to [`SessionConcurrencyPolicy::AllowConcurrent`] when unset.
    pub session_concurrency: Option<SessionConcurrencyPolicy>,
    /// Optional intra-invocation compaction configuration.
    /// When set, the runner estimates token count before each agent run
    /// and triggers mid-invocation summarization when the threshold is exceeded.
    pub intra_compaction_config: Option<adk_core::IntraCompactionConfig>,
    /// Optional summarizer for intra-invocation compaction.
    /// Required when `intra_compaction_config` is set.
    pub intra_compaction_summarizer: Option<Arc<dyn adk_core::BaseEventsSummarizer>>,
    /// Optional context compaction configuration for token-budget overflow handling.
    ///
    /// When set, the runner applies the configured [`CompactionStrategy`](crate::compaction::CompactionStrategy)
    /// to shrink the event history when the context exceeds the token budget,
    /// retrying the model request up to `max_retries` times.
    ///
    /// This field is only available when the `context-compaction` feature is enabled.
    #[cfg(feature = "context-compaction")]
    pub context_compaction: Option<crate::compaction::CompactionConfig>,
}

/// Agent execution runtime.
///
/// Orchestrates session retrieval, agent dispatch, event streaming, context
/// caching, and compaction. Construct via [`Runner::builder()`] or
/// [`Runner::new()`].
pub struct Runner {
    app_name: String,
    root_agent: Arc<dyn Agent>,
    session_service: Arc<dyn SessionService>,
    #[cfg(feature = "artifacts")]
    artifact_service: Option<Arc<dyn ArtifactService>>,
    memory_service: Option<Arc<dyn Memory>>,
    #[cfg(feature = "plugins")]
    plugin_manager: Option<Arc<PluginManager>>,
    #[cfg(feature = "skills")]
    skill_injector: Option<Arc<SkillInjector>>,
    run_config: RunConfig,
    compaction_config: Option<adk_core::EventsCompactionConfig>,
    context_cache_config: Option<ContextCacheConfig>,
    cache_capable: Option<Arc<dyn CacheCapable>>,
    cache_manager: Option<Arc<tokio::sync::Mutex<CacheManager>>>,
    request_context: Option<adk_core::RequestContext>,
    cancellation_token: Option<CancellationToken>,
    intra_compactor: Option<Arc<crate::intra_compaction::IntraInvocationCompactor>>,
    /// Optional context compaction configuration for token-budget overflow handling.
    #[cfg(feature = "context-compaction")]
    context_compaction: Option<Arc<crate::compaction::CompactionConfig>>,
    /// In-flight invocation registry (invocation-id keyed + session index).
    /// Each run registers its invocation-scoped token here from *inside* its
    /// event stream; `interrupt*` cancels the matching token(s).
    registry: Arc<std::sync::Mutex<Registry>>,
    /// Session concurrency policy governing same-session runs.
    session_concurrency: SessionConcurrencyPolicy,
    /// Per-session serialization gates (used only under `Serialize`).
    session_gates: Arc<std::sync::Mutex<SessionGates>>,
}

impl Runner {
    /// Create a typestate builder for constructing a `Runner`.
    ///
    /// The builder enforces at compile time that the three required fields
    /// (`app_name`, `agent`, `session_service`) are set before `build()` is
    /// callable.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = Runner::builder()
    ///     .app_name("my-app")
    ///     .agent(agent)
    ///     .session_service(session_service)
    ///     .build()?;
    /// ```
    pub fn builder() -> crate::builder::RunnerConfigBuilder<
        crate::builder::NoAppName,
        crate::builder::NoAgent,
        crate::builder::NoSessionService,
    > {
        crate::builder::RunnerConfigBuilder::new()
    }

    /// Create a new runner from a [`RunnerConfig`].
    ///
    /// Prefer [`Runner::builder()`] for a compile-time-safe construction API.
    pub fn new(config: RunnerConfig) -> Result<Self> {
        let run_config = config.run_config.unwrap_or_default();

        // When a cache-capable model is provided but no explicit cache config,
        // use the default ContextCacheConfig to enable caching automatically.
        let effective_cache_config = config
            .context_cache_config
            .or_else(|| config.cache_capable.as_ref().map(|_| ContextCacheConfig::default()));

        let cache_manager = effective_cache_config
            .as_ref()
            .map(|c| Arc::new(tokio::sync::Mutex::new(CacheManager::new(c.clone()))));

        let intra_compactor = config.intra_compaction_config.as_ref().and_then(|ic_config| {
            config.intra_compaction_summarizer.as_ref().map(|summarizer| {
                Arc::new(crate::intra_compaction::IntraInvocationCompactor::new(
                    ic_config.clone(),
                    summarizer.clone(),
                ))
            })
        });

        let session_concurrency = config.session_concurrency.unwrap_or_default();
        let gate_capacity = match session_concurrency {
            SessionConcurrencyPolicy::Serialize { retained_sessions, .. } => retained_sessions,
            _ => 1,
        };

        Ok(Self {
            app_name: config.app_name,
            root_agent: config.agent,
            session_service: config.session_service,
            #[cfg(feature = "artifacts")]
            artifact_service: config.artifact_service,
            memory_service: config.memory_service,
            #[cfg(feature = "plugins")]
            plugin_manager: config.plugin_manager,
            #[cfg(feature = "skills")]
            skill_injector: None,
            run_config,
            compaction_config: config.compaction_config,
            context_cache_config: effective_cache_config,
            cache_capable: config.cache_capable,
            cache_manager,
            request_context: config.request_context,
            cancellation_token: config.cancellation_token,
            intra_compactor,
            #[cfg(feature = "context-compaction")]
            context_compaction: config.context_compaction.map(Arc::new),
            registry: Arc::new(std::sync::Mutex::new(Registry::default())),
            session_concurrency,
            session_gates: Arc::new(std::sync::Mutex::new(SessionGates::new(gate_capacity))),
        })
    }

    /// Enable skill injection using a pre-built injector.
    ///
    /// Skill injection runs before plugin `on_user_message` callbacks.
    #[cfg(feature = "skills")]
    pub fn with_skill_injector(mut self, injector: SkillInjector) -> Self {
        self.skill_injector = Some(Arc::new(injector));
        self
    }

    /// Enable skill injection by auto-loading `.skills/` from the given root path.
    #[cfg(feature = "skills")]
    #[deprecated(note = "Use with_auto_skills_mut instead")]
    pub fn with_auto_skills(
        mut self,
        root: impl AsRef<std::path::Path>,
        config: SkillInjectorConfig,
    ) -> adk_skill::SkillResult<Self> {
        self.with_auto_skills_mut(root, config)?;
        Ok(self)
    }

    /// Enable skill injection by auto-loading `.skills/` from the given root path.
    ///
    /// Unlike [`with_auto_skills`](Self::with_auto_skills), this method borrows
    /// the Runner mutably instead of consuming it. On error, the Runner remains
    /// valid with no skill injector configured.
    #[cfg(feature = "skills")]
    pub fn with_auto_skills_mut(
        &mut self,
        root: impl AsRef<std::path::Path>,
        config: SkillInjectorConfig,
    ) -> adk_skill::SkillResult<()> {
        let injector = SkillInjector::from_root(root, config)?;
        self.skill_injector = Some(Arc::new(injector));
        Ok(())
    }

    /// Start an agent run, returning a [`RunHandle`].
    ///
    /// Synchronous: performs the single eager fallible step (`AppName` validation),
    /// mints the invocation id (kept in the `"inv-<uuid>"` format used by telemetry
    /// spans), derives the invocation-scoped cancellation token, and builds the lazy
    /// event stream. **All run lifecycle** — registry registration/cleanup and the
    /// serialization permit — is anchored *inside* the returned
    /// [`RunHandle::events`] stream via RAII (never in a handle field), so discarding
    /// the handle (as [`run`](Self::run) does) never orphans state.
    pub fn start(
        &self,
        user_id: UserId,
        session_id: SessionId,
        user_content: Content,
    ) -> Result<RunHandle> {
        // Eager, fallible: validate the app name up front (the one fallible step
        // that must surface synchronously to the caller).
        let typed_app_name = AppName::try_from(self.app_name.clone())?;

        // Mint the invocation id eagerly: it is validated by `InvocationId::try_from`
        // inside the context builder and drives the telemetry span attributes, so it
        // must be stable across the whole run.
        let invocation_id = format!("inv-{}", uuid::Uuid::new_v4());

        // Derive the invocation-scoped cancellation token. When a global token is
        // configured, make this a CHILD of it (`child_token`) so global cancellation
        // propagates here with ZERO spawned tasks; `interrupt*` cancels only this run.
        //
        // (FClaw memory fix — see docs/ADK_FORK.md. The previous implementation spawned
        // two watcher tasks per run to union the global + session tokens, but never
        // aborted them; `child_token` expresses the same "cancel on either" union with
        // no tasks at all.)
        let session_token = match &self.cancellation_token {
            Some(global) => global.child_token(),
            None => CancellationToken::new(),
        };

        let events = self.build_event_stream(
            invocation_id.clone(),
            typed_app_name,
            session_token.clone(),
            user_id,
            session_id.clone(),
            user_content,
        );

        Ok(RunHandle { invocation_id, session_id, events, cancellation: session_token })
    }

    /// Execute the root agent for the given user and session, returning an event stream.
    ///
    /// Retrieves (or creates) the session, resolves the target agent, runs
    /// plugins/skills, and streams events as the agent executes.
    ///
    /// This is a thin shim over [`start`](Self::start) — it discards the
    /// [`RunHandle`] and returns only its event stream. Because all lifecycle
    /// cleanup lives inside the stream, discarding the handle is safe.
    pub async fn run(
        &self,
        user_id: UserId,
        session_id: SessionId,
        user_content: Content,
    ) -> Result<EventStream> {
        Ok(self.start(user_id, session_id, user_content)?.events)
    }

    /// Build the lazy event stream for a run whose invocation id and cancellation
    /// token were minted by [`start`](Self::start).
    ///
    /// The registry registration + [`SessionCleanup`] guard and (under `Serialize`)
    /// the serialization permit are all anchored inside the returned stream, so they
    /// are created only when the stream is first polled and released by RAII when it
    /// ends or is dropped.
    fn build_event_stream(
        &self,
        invocation_id: String,
        typed_app_name: AppName,
        session_token: CancellationToken,
        user_id: UserId,
        session_id: SessionId,
        user_content: Content,
    ) -> EventStream {
        let app_name = self.app_name.clone();
        let session_service = self.session_service.clone();
        let root_agent = self.root_agent.clone();
        #[cfg(feature = "artifacts")]
        let artifact_service = self.artifact_service.clone();
        let memory_service = self.memory_service.clone();
        #[cfg(feature = "plugins")]
        let plugin_manager = self.plugin_manager.clone();
        #[cfg(feature = "skills")]
        let skill_injector = self.skill_injector.clone();
        let mut run_config = self.run_config.clone();
        let compaction_config = self.compaction_config.clone();
        let context_cache_config = self.context_cache_config.clone();
        let cache_capable = self.cache_capable.clone();
        let cache_manager_ref = self.cache_manager.clone();
        let request_context = self.request_context.clone();
        let intra_compactor = self.intra_compactor.clone();
        #[cfg(feature = "context-compaction")]
        let context_compaction = self.context_compaction.clone();

        let registry = self.registry.clone();
        let session_concurrency = self.session_concurrency;
        let session_gates = self.session_gates.clone();

        let session_id_str = session_id.as_str().to_string();

        // The invocation-scoped token unifies global + per-invocation cancellation
        // (it is a child of the global token when one is configured), so it IS the
        // effective token — no combined token, no watcher tasks.
        let effective_token = Some(session_token.clone());

        let s = stream! {
            // ===== REGISTRY REGISTRATION + RAII CLEANUP =====
            // Registration lives INSIDE the stream (not in start()) so a handle whose
            // stream is never polled never leaks a registry entry. The Drop guard
            // removes both the invocation-keyed token and the session index entry.
            struct SessionCleanup {
                registry: Arc<std::sync::Mutex<Registry>>,
                invocation_id: String,
                session_id: String,
            }
            impl Drop for SessionCleanup {
                fn drop(&mut self) {
                    let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
                    reg.active.remove(&self.invocation_id);
                    if let Some(ids) = reg.by_session.get_mut(&self.session_id) {
                        ids.retain(|id| id != &self.invocation_id);
                        if ids.is_empty() {
                            reg.by_session.remove(&self.session_id);
                        }
                    }
                }
            }

            // RejectConcurrent: refuse before registering if the session already has an
            // in-flight run. Otherwise register (invocation-keyed token + FIFO session
            // index) so distinct same-session runs never overwrite each other. The lock
            // guard is fully released (the block yields a plain bool) BEFORE any yield/
            // await, so no MutexGuard is ever held across a suspension point.
            let rejected = {
                let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                if matches!(session_concurrency, SessionConcurrencyPolicy::RejectConcurrent)
                    && reg.by_session.get(&session_id_str).is_some_and(|v| !v.is_empty())
                {
                    true
                } else {
                    reg.active.insert(invocation_id.clone(), session_token.clone());
                    reg.by_session
                        .entry(session_id_str.clone())
                        .or_default()
                        .push(invocation_id.clone());
                    false
                }
            };
            if rejected {
                tracing::debug!(
                    session.id = %session_id_str,
                    "rejecting concurrent run (RejectConcurrent policy)"
                );
                yield Err(session_busy_error());
                return;
            }
            let _cleanup = SessionCleanup {
                registry: registry.clone(),
                invocation_id: invocation_id.clone(),
                session_id: session_id_str.clone(),
            };

            // ===== SESSION SERIALIZATION GATE (Serialize policy only) =====
            // Hold an OwnedSemaphorePermit for the WHOLE run so same-session runs
            // execute one-at-a-time (fair FIFO), while different sessions use different
            // semaphores and run in parallel. The permit (and the depth guard) are held
            // in this stream local and released by RAII on stream end/return.
            let _serialize_permit: Option<(OwnedSemaphorePermit, DepthGuard)>;
            if let SessionConcurrencyPolicy::Serialize { max_queue, .. } = session_concurrency {
                let gate = {
                    let mut gates = session_gates.lock().unwrap_or_else(|e| e.into_inner());
                    gates.get_or_create(&session_id_str)
                };
                // depth = holder + waiters. `depth_before` runs are already in the gate
                // region; this run would become waiter #depth_before (one in-region run
                // holds the permit). Reject when that would exceed max_queue.
                let depth_before = gate.depth.fetch_add(1, Ordering::SeqCst);
                if depth_before > max_queue {
                    gate.depth.fetch_sub(1, Ordering::SeqCst);
                    tracing::debug!(
                        session.id = %session_id_str,
                        max_queue,
                        "rejecting run (Serialize max_queue exceeded)"
                    );
                    yield Err(session_busy_error());
                    return; // _cleanup drops → registry entry removed
                }
                let depth_guard = DepthGuard { depth: gate.depth.clone() };

                // A queued run stays cancellable (via interrupt* or stream drop) while it
                // waits; the semaphore is fair ⇒ FIFO ordering.
                let permit = tokio::select! {
                    biased;
                    _ = session_token.cancelled() => {
                        tracing::debug!(
                            session.id = %session_id_str,
                            "queued run cancelled before acquiring session gate"
                        );
                        return; // depth_guard + _cleanup drop → all state released
                    }
                    res = gate.sem.clone().acquire_owned() => {
                        match res {
                            Ok(p) => p,
                            Err(_) => return, // semaphore closed (never, in practice)
                        }
                    }
                };
                _serialize_permit = Some((permit, depth_guard));
            } else {
                _serialize_permit = None;
            }

            // Use the effective token (global + per-invocation via child token)
            let cancellation_token = effective_token;
            // Get or create session
            let session = match session_service
                .get(adk_session::GetRequest {
                    app_name: app_name.clone(),
                    user_id: user_id.to_string(),
                    session_id: session_id.to_string(),
                    num_recent_events: run_config.history_max_events,
                    after: None,
                })
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            // Find which agent should handle this request
            let agent_to_run = Self::find_agent_to_run(&root_agent, session.as_ref());

            // Clone services for potential reuse in transfer
            #[cfg(feature = "artifacts")]
            let artifact_service_clone = artifact_service.clone();
            let memory_service_clone = memory_service.clone();

            // Create invocation context with MutableSession. `invocation_id` was
            // minted eagerly in `start()` and captured into this stream.
            #[cfg(any(feature = "skills", feature = "plugins"))]
            let mut effective_user_content = user_content.clone();
            #[cfg(not(any(feature = "skills", feature = "plugins")))]
            let effective_user_content = user_content.clone();
            #[cfg(feature = "skills")]
            let mut selected_skill_name = String::new();
            #[cfg(not(feature = "skills"))]
            let selected_skill_name = String::new();
            #[cfg(feature = "skills")]
            let mut selected_skill_id = String::new();
            #[cfg(not(feature = "skills"))]
            let selected_skill_id = String::new();

            #[cfg(feature = "skills")]
            if let Some(injector) = skill_injector.as_ref()
                && let Some(matched) = adk_skill::apply_skill_injection(
                    &mut effective_user_content,
                    injector.index(),
                    injector.policy(),
                    injector.max_injected_chars(),
                ) {
                    selected_skill_name = matched.skill.name;
                    selected_skill_id = matched.skill.id;
                }

            let mut invocation_ctx = match InvocationContext::new_typed(
                invocation_id.clone(),
                agent_to_run.clone(),
                user_id.clone(),
                typed_app_name.clone(),
                session_id.clone(),
                effective_user_content.clone(),
                Arc::from(session),
            ) {
                Ok(ctx) => ctx,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            // Add optional services
            #[cfg(feature = "artifacts")]
            if let Some(service) = artifact_service {
                // Wrap service with ScopedArtifacts to bind session context
                let scoped = adk_artifact::ScopedArtifacts::new(
                    service,
                    app_name.clone(),
                    user_id.to_string(),
                    session_id.to_string(),
                );
                invocation_ctx = invocation_ctx.with_artifacts(Arc::new(scoped));
            }
            if let Some(memory) = memory_service {
                invocation_ctx = invocation_ctx.with_memory(memory);
            }

            // Apply run config (streaming mode, etc.)
            invocation_ctx = invocation_ctx.with_run_config(run_config.clone());

            // Apply request context from auth middleware bridge if present
            if let Some(rc) = request_context.clone() {
                invocation_ctx = invocation_ctx.with_request_context(rc);
            }

            // Thread the effective per-session cancellation token so tools can
            // observe interruption via `ToolContext::cancellation_token()`.
            if let Some(token) = cancellation_token.clone() {
                invocation_ctx = invocation_ctx.with_cancellation_token(token);
            }

            let mut ctx = Arc::new(invocation_ctx);

            #[cfg(feature = "plugins")]
            if let Some(manager) = plugin_manager.as_ref() {
                match manager
                    .run_before_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>)
                    .await
                {
                    Ok(Some(content)) => {
                        let mut early_event = adk_core::Event::new(ctx.invocation_id());
                        early_event.author = agent_to_run.name().to_string();
                        early_event.llm_response.content = Some(content);

                        ctx.mutable_session().append_event(early_event.clone());
                        if let Err(e) = session_service.append_event(ctx.session_id(), early_event.clone()).await {
                            yield Err(e);
                            return;
                        }

                        yield Ok(early_event);
                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        return;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        yield Err(e);
                        return;
                    }
                }

                match manager
                    .run_on_user_message(
                        ctx.clone() as Arc<dyn adk_core::InvocationContext>,
                        effective_user_content.clone(),
                    )
                    .await
                {
                    Ok(Some(modified)) => {
                        effective_user_content = modified;

                        let mut refreshed_ctx = match InvocationContext::with_mutable_session(
                            ctx.invocation_id().to_string(),
                            agent_to_run.clone(),
                            ctx.user_id().to_string(),
                            ctx.app_name().to_string(),
                            ctx.session_id().to_string(),
                            effective_user_content.clone(),
                            ctx.mutable_session().clone(),
                        ) {
                            Ok(ctx) => ctx,
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        };

                        #[cfg(feature = "artifacts")]
                        if let Some(service) = artifact_service_clone.clone() {
                            let scoped = adk_artifact::ScopedArtifacts::new(
                                service,
                                ctx.app_name().to_string(),
                                ctx.user_id().to_string(),
                                ctx.session_id().to_string(),
                            );
                            refreshed_ctx = refreshed_ctx.with_artifacts(Arc::new(scoped));
                        }
                        if let Some(memory) = memory_service_clone.clone() {
                            refreshed_ctx = refreshed_ctx.with_memory(memory);
                        }
                        refreshed_ctx = refreshed_ctx.with_run_config(run_config.clone());
                        if let Some(rc) = request_context.clone() {
                            refreshed_ctx = refreshed_ctx.with_request_context(rc);
                        }
                        if let Some(token) = cancellation_token.clone() {
                            refreshed_ctx = refreshed_ctx.with_cancellation_token(token);
                        }
                        ctx = Arc::new(refreshed_ctx);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        if let Some(manager) = plugin_manager.as_ref() {
                            manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        }
                        yield Err(e);
                        return;
                    }
                }
            }

            // Append user message to session service (persistent storage)
            let mut user_event = adk_core::Event::new(ctx.invocation_id());
            user_event.author = "user".to_string();
            user_event.llm_response.content = Some(effective_user_content.clone());

            // Also add to mutable session for immediate visibility
            // Note: adk_session::Event is a re-export of adk_core::Event, so we can use it directly
            ctx.mutable_session().append_event(user_event.clone());

            if let Err(e) = session_service.append_event(ctx.session_id(), user_event).await {
                #[cfg(feature = "plugins")]
                if let Some(manager) = plugin_manager.as_ref() {
                    manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                }
                yield Err(e);
                return;
            }

            // ===== CONTEXT CACHE LIFECYCLE =====
            // If context caching is configured and a cache-capable model is available,
            // create or refresh the cached content before agent execution.
            // Cache failures are non-fatal — log a warning and proceed without cache.
            if let (Some(cm_mutex), Some(cache_model)) = (&cache_manager_ref, &cache_capable) {
                let should_refresh_cache = {
                    let cm = cm_mutex.lock().await;
                    cm.is_enabled() && (cm.active_cache_name().is_none() || cm.needs_refresh())
                };

                if should_refresh_cache {
                    // Gather system instruction from the agent's description
                    // (the full instruction is resolved inside the agent, but the
                    // description provides a reasonable proxy for cache keying).
                    let system_instruction = agent_to_run.description().to_string();
                    let tools = std::collections::HashMap::new();
                    let ttl = context_cache_config.as_ref().map_or(600, |c| c.ttl_seconds);

                    match cache_model.create_cache(&system_instruction, &tools, ttl).await {
                        Ok(name) => {
                            let old_cache = {
                                let mut cm = cm_mutex.lock().await;
                                let old = cm.clear_active_cache();
                                cm.set_active_cache(name);
                                old
                            };

                            if let Some(old) = old_cache
                                && let Err(e) = cache_model.delete_cache(&old).await {
                                    tracing::warn!(
                                        old_cache = %old,
                                        error = %e,
                                        "failed to delete old cache, proceeding with new cache"
                                    );
                                }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "cache creation failed, proceeding without cache"
                            );
                        }
                    }
                }

                // Attach cache name to run config so agents can use it.
                let cache_name = {
                    let mut cm = cm_mutex.lock().await;
                    if cm.is_enabled() {
                        cm.record_invocation().map(str::to_string)
                    } else {
                        None
                    }
                };

                if let Some(cache_name) = cache_name {
                    run_config.cached_content = Some(cache_name);
                    // Rebuild the invocation context with the updated run config.
                    let mut refreshed_ctx = match InvocationContext::with_mutable_session(
                        ctx.invocation_id().to_string(),
                        agent_to_run.clone(),
                        ctx.user_id().to_string(),
                        ctx.app_name().to_string(),
                        ctx.session_id().to_string(),
                        effective_user_content.clone(),
                        ctx.mutable_session().clone(),
                    ) {
                        Ok(ctx) => ctx,
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    };
                    #[cfg(feature = "artifacts")]
                    if let Some(service) = artifact_service_clone.clone() {
                        let scoped = adk_artifact::ScopedArtifacts::new(
                            service,
                            ctx.app_name().to_string(),
                            ctx.user_id().to_string(),
                            ctx.session_id().to_string(),
                        );
                        refreshed_ctx = refreshed_ctx.with_artifacts(Arc::new(scoped));
                    }
                    if let Some(memory) = memory_service_clone.clone() {
                        refreshed_ctx = refreshed_ctx.with_memory(memory);
                    }
                    refreshed_ctx = refreshed_ctx.with_run_config(run_config.clone());
                    if let Some(rc) = request_context.clone() {
                        refreshed_ctx = refreshed_ctx.with_request_context(rc);
                    }
                    if let Some(token) = cancellation_token.clone() {
                        refreshed_ctx = refreshed_ctx.with_cancellation_token(token);
                    }
                    ctx = Arc::new(refreshed_ctx);
                }
            }

            // ===== INTRA-INVOCATION COMPACTION =====
            // If intra-compaction is configured, check if the session events
            // exceed the token threshold and compact them before the agent runs.
            if let Some(ref compactor) = intra_compactor {
                compactor.reset_cycle();
                let session_events = ctx.mutable_session().as_ref().events_snapshot();
                match compactor.maybe_compact(&session_events).await {
                    Ok(Some(compacted_events)) => {
                        ctx.mutable_session().replace_events(compacted_events);
                        tracing::info!("intra-invocation compaction applied before agent execution");
                    }
                    Ok(None) => {} // No compaction needed
                    Err(e) => {
                        tracing::warn!(error = %e, "intra-invocation compaction check failed");
                    }
                }
            }

            // ===== CONTEXT COMPACTION (TOKEN BUDGET) =====
            // If context-compaction is configured, proactively check the estimated
            // token count before calling the agent. If it exceeds the budget,
            // apply compaction to bring it under the limit.
            #[cfg(feature = "context-compaction")]
            if let Some(ref cc_config) = context_compaction {
                let session_events = ctx.mutable_session().events_snapshot();
                let estimated = crate::compaction::estimate_event_tokens(&session_events);
                if estimated > cc_config.context_budget {
                    tracing::info!(
                        estimated_tokens = estimated,
                        budget = cc_config.context_budget,
                        "context exceeds budget, applying proactive compaction"
                    );
                    match crate::compaction::apply_compaction_with_retry(cc_config, session_events).await {
                        Ok(compacted) => {
                            ctx.mutable_session().replace_events(compacted);
                            tracing::info!("proactive context compaction succeeded");
                        }
                        Err(e) => {
                            // Proactive compaction failed — proceed anyway and let the
                            // model reject the request if it's truly too large.
                            tracing::warn!(error = %e, "proactive context compaction failed, proceeding with full context");
                        }
                    }
                }
            }

            // Run the agent with instrumentation (ADK-Go style attributes)
            let agent_span = tracing::info_span!(
                "agent.execute",
                "gcp.vertex.agent.invocation_id" = ctx.invocation_id(),
                "gcp.vertex.agent.session_id" = ctx.session_id(),
                "gcp.vertex.agent.event_id" = ctx.invocation_id(), // Use invocation_id as event_id for agent spans
                "gen_ai.conversation.id" = ctx.session_id(),
                "adk.app_name" = ctx.app_name(),
                "adk.user_id" = ctx.user_id(),
                "agent.name" = %agent_to_run.name(),
                "adk.skills.selected_name" = %selected_skill_name,
                "adk.skills.selected_id" = %selected_skill_id
            );

            let mut agent_stream = match agent_to_run.run(ctx.clone()).instrument(agent_span.clone()).await {
                Ok(s) => s,
                #[cfg(feature = "context-compaction")]
                Err(e) if context_compaction.is_some() && crate::compaction::is_token_limit_error(&e) => {
                    // Token limit error on agent.run() — apply compaction and retry
                    let cc_config = context_compaction.as_ref().unwrap();
                    tracing::warn!(
                        error = %e,
                        "agent execution failed with token limit error, attempting compaction"
                    );
                    let session_events = ctx.mutable_session().events_snapshot();
                    match crate::compaction::apply_compaction_with_retry(cc_config, session_events).await {
                        Ok(compacted) => {
                            ctx.mutable_session().replace_events(compacted);
                            tracing::info!("context compaction succeeded after token limit error, retrying agent");
                            // Retry the agent call with compacted context
                            match agent_to_run.run(ctx.clone()).instrument(agent_span).await {
                                Ok(s) => s,
                                Err(retry_err) => {
                                    #[cfg(feature = "plugins")]
                                    if let Some(manager) = plugin_manager.as_ref() {
                                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                                    }
                                    yield Err(retry_err);
                                    return;
                                }
                            }
                        }
                        Err(compaction_err) => {
                            #[cfg(feature = "plugins")]
                            if let Some(manager) = plugin_manager.as_ref() {
                                manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                            }
                            yield Err(compaction_err);
                            return;
                        }
                    }
                }
                Err(e) => {
                    #[cfg(feature = "plugins")]
                    if let Some(manager) = plugin_manager.as_ref() {
                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                    }
                    yield Err(e);
                    return;
                }
            };

            // Stream events and check for transfers
            use futures::StreamExt;
            let mut transfer_target: Option<String> = None;

            while let Some(result) = {
                if let Some(token) = cancellation_token.as_ref()
                    && token.is_cancelled() {
                        #[cfg(feature = "plugins")]
                        if let Some(manager) = plugin_manager.as_ref() {
                            manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        }
                        return;
                    }
                agent_stream.next().await
            } {
                match result {
                    Ok(event) => {
                        #[cfg(feature = "plugins")]
                        let mut event = event;

                        #[cfg(feature = "plugins")]
                        if let Some(manager) = plugin_manager.as_ref() {
                            match manager
                                .run_on_event(
                                    ctx.clone() as Arc<dyn adk_core::InvocationContext>,
                                    event.clone(),
                                )
                                .await
                            {
                                Ok(Some(modified)) => {
                                    event = modified;
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                                    yield Err(e);
                                    return;
                                }
                            }
                        }

                        // Check for transfer action
                        if let Some(target) = &event.actions.transfer_to_agent {
                            transfer_target = Some(target.clone());
                        }

                        // CRITICAL: Apply state_delta to the mutable session immediately.
                        // This is the key fix for state propagation between sequential agents.
                        // When an agent sets output_key, it emits an event with state_delta.
                        // We must apply this to the mutable session so downstream agents
                        // can read the value via ctx.session().state().get().
                        if !event.actions.state_delta.is_empty() {
                            ctx.mutable_session().apply_state_delta(&event.actions.state_delta);
                        }

                        // Also add the event to the mutable session's event list — but SKIP partial
                        // streaming chunks. They are ephemeral deltas (the terminal, non-partial
                        // event carries the accumulated content and IS persisted below); retaining
                        // every partial in the in-RAM session, each carrying a cloned full request,
                        // grew memory by request_size × chunk_count per streamed turn. FClaw memory
                        // fix — see docs/ADK_FORK.md.
                        if !event.llm_response.partial {
                            ctx.mutable_session().append_event(event.clone());
                        }

                        // Append event to session service (persistent storage)
                        // Skip partial streaming chunks — only persist the final
                        // event. Streaming chunks share the same event ID, so
                        // persisting each one would violate the primary key
                        // constraint. The final chunk (partial=false) carries the
                        // complete accumulated content.
                        if !event.llm_response.partial
                            && let Err(e) = session_service.append_event(ctx.session_id(), event.clone()).await {
                                #[cfg(feature = "plugins")]
                                if let Some(manager) = plugin_manager.as_ref() {
                                    manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                                }
                                yield Err(e);
                                return;
                            }
                        yield Ok(event);
                    }
                    Err(e) => {
                        #[cfg(feature = "plugins")]
                        if let Some(manager) = plugin_manager.as_ref() {
                            manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        }
                        yield Err(e);
                        return;
                    }
                }
            }

            // ===== TRANSFER LOOP =====
            // Support multi-hop transfers with a max-depth guard.
            // When an agent emits transfer_to_agent, the runner resolves the
            // target from the root agent tree, computes transfer_targets
            // (parent + peers) for the new agent, and runs it. This repeats
            // until no further transfer is requested or the depth limit is hit.
            const DEFAULT_MAX_TRANSFER_DEPTH: u32 = 10;
            let max_depth = run_config.max_transfer_depth.unwrap_or(DEFAULT_MAX_TRANSFER_DEPTH);
            let mut transfer_depth: u32 = 0;
            let mut current_transfer_target = transfer_target;

            while let Some(target_name) = current_transfer_target.take() {
                transfer_depth += 1;
                if transfer_depth > max_depth {
                    tracing::warn!(
                        depth = transfer_depth,
                        target = %target_name,
                        "max transfer depth exceeded, stopping transfer chain"
                    );
                    break;
                }

                let target_agent = match Self::find_agent(&root_agent, &target_name) {
                    Some(a) => a,
                    None => {
                        tracing::warn!(target = %target_name, "transfer target not found in agent tree");
                        break;
                    }
                };

                // Compute transfer_targets for the target agent:
                // - parent: the agent that transferred to it (or root if applicable)
                // - peers: siblings in the agent tree
                // - children: handled by the agent itself via sub_agents()
                let (parent_name, peer_names) = Self::compute_transfer_context(&root_agent, &target_name);

                let mut transfer_run_config = run_config.clone();
                let mut targets = Vec::new();
                if let Some(ref parent) = parent_name {
                    targets.push(parent.clone());
                }
                targets.extend(peer_names);
                transfer_run_config.transfer_targets = targets;
                transfer_run_config.parent_agent = parent_name;

                // For transfers, we reuse the same mutable session to preserve state
                let transfer_invocation_id = format!("inv-{}", uuid::Uuid::new_v4());
                let mut transfer_ctx = match InvocationContext::with_mutable_session(
                    transfer_invocation_id.clone(),
                    target_agent.clone(),
                    ctx.user_id().to_string(),
                    ctx.app_name().to_string(),
                    ctx.session_id().to_string(),
                    effective_user_content.clone(),
                    ctx.mutable_session().clone(),
                ) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                };

                #[cfg(feature = "artifacts")]
                if let Some(ref service) = artifact_service_clone {
                    let scoped = adk_artifact::ScopedArtifacts::new(
                        service.clone(),
                        ctx.app_name().to_string(),
                        ctx.user_id().to_string(),
                        ctx.session_id().to_string(),
                    );
                    transfer_ctx = transfer_ctx.with_artifacts(Arc::new(scoped));
                }
                if let Some(ref memory) = memory_service_clone {
                    transfer_ctx = transfer_ctx.with_memory(memory.clone());
                }
                transfer_ctx = transfer_ctx.with_run_config(transfer_run_config);
                if let Some(rc) = request_context.clone() {
                    transfer_ctx = transfer_ctx.with_request_context(rc);
                }
                if let Some(token) = cancellation_token.clone() {
                    transfer_ctx = transfer_ctx.with_cancellation_token(token);
                }

                let transfer_ctx = Arc::new(transfer_ctx);

                // Run the transferred agent
                let mut transfer_stream = match target_agent.run(transfer_ctx.clone()).await {
                    Ok(s) => s,
                    Err(e) => {
                        #[cfg(feature = "plugins")]
                        if let Some(manager) = plugin_manager.as_ref() {
                            manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                        }
                        yield Err(e);
                        return;
                    }
                };

                // Stream events from the transferred agent, capturing any further transfer
                while let Some(result) = {
                    if let Some(token) = cancellation_token.as_ref()
                        && token.is_cancelled() {
                            #[cfg(feature = "plugins")]
                            if let Some(manager) = plugin_manager.as_ref() {
                                manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                            }
                            return;
                        }
                    transfer_stream.next().await
                } {
                    match result {
                        Ok(event) => {
                            #[cfg(feature = "plugins")]
                            let mut event = event;
                            #[cfg(feature = "plugins")]
                            if let Some(manager) = plugin_manager.as_ref() {
                                match manager
                                    .run_on_event(
                                        transfer_ctx.clone() as Arc<dyn adk_core::InvocationContext>,
                                        event.clone(),
                                    )
                                    .await
                                {
                                    Ok(Some(modified)) => {
                                        event = modified;
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                                        yield Err(e);
                                        return;
                                    }
                                }
                            }

                            // Capture further transfer requests
                            if let Some(target) = &event.actions.transfer_to_agent {
                                current_transfer_target = Some(target.clone());
                            }

                            // Apply state delta for transferred agent too
                            if !event.actions.state_delta.is_empty() {
                                transfer_ctx.mutable_session().apply_state_delta(&event.actions.state_delta);
                            }

                            // Add to mutable session — skip partial streaming chunks (see the main
                            // loop above; FClaw memory fix, docs/ADK_FORK.md).
                            if !event.llm_response.partial {
                                transfer_ctx.mutable_session().append_event(event.clone());
                            }

                            if !event.llm_response.partial
                                && let Err(e) = session_service.append_event(ctx.session_id(), event.clone()).await {
                                    #[cfg(feature = "plugins")]
                                    if let Some(manager) = plugin_manager.as_ref() {
                                        manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                                    }
                                    yield Err(e);
                                    return;
                                }
                            yield Ok(event);
                        }
                        Err(e) => {
                            #[cfg(feature = "plugins")]
                            if let Some(manager) = plugin_manager.as_ref() {
                                manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
                            }
                            yield Err(e);
                            return;
                        }
                    }
                }
            }

            // ===== CONTEXT COMPACTION =====
            // After all events have been processed, check if compaction should trigger.
            // This runs in the background after the invocation completes.
            if let Some(ref compaction_cfg) = compaction_config {
                let event_count = ctx.mutable_session().as_ref().events_len();

                if event_count > 0 {
                    let all_events = ctx.mutable_session().as_ref().events_snapshot();
                    let invocation_count = all_events.iter().filter(|e| e.author == "user").count()
                        as u32;

                    if invocation_count > 0
                        && invocation_count.is_multiple_of(compaction_cfg.compaction_interval)
                    {
                        // Determine the window of events to compact
                        // We compact all events except the most recent overlap_size invocations
                        let overlap = compaction_cfg.overlap_size as usize;

                        // Find the boundary: keep the last `overlap` user messages and everything after
                        let user_msg_indices: Vec<usize> = all_events.iter()
                            .enumerate()
                            .filter(|(_, e)| e.author == "user")
                            .map(|(i, _)| i)
                            .collect();

                        // Keep the last `overlap` user messages intact.
                        // When overlap is 0, compact everything.
                        let compact_up_to = if overlap == 0 {
                            all_events.len()
                        } else if user_msg_indices.len() > overlap {
                            // Compact up to (but not including) the overlap-th-from-last user message
                            user_msg_indices[user_msg_indices.len() - overlap]
                        } else {
                            // Not enough user messages to satisfy overlap — skip compaction
                            0
                        };

                        if compact_up_to > 0 {
                            let events_to_compact = &all_events[..compact_up_to];

                            match compaction_cfg.summarizer.summarize_events(events_to_compact).await {
                                Ok(Some(compaction_event)) => {
                                    // Persist the compaction event
                                    if let Err(e) = session_service.append_event(
                                        ctx.session_id(),
                                        compaction_event.clone(),
                                    ).await {
                                        tracing::warn!(error = %e, "Failed to persist compaction event");
                                    } else {
                                        tracing::info!(
                                            compacted_events = compact_up_to,
                                            "Context compaction completed"
                                        );
                                    }
                                }
                                Ok(None) => {
                                    tracing::debug!("Compaction summarizer returned no result");
                                }
                                Err(e) => {
                                    // Compaction failure is non-fatal — log and continue
                                    tracing::warn!(error = %e, "Context compaction failed");
                                }
                            }
                        }
                    }
                }
            }

            #[cfg(feature = "plugins")]
            if let Some(manager) = plugin_manager.as_ref() {
                manager.run_after_run(ctx.clone() as Arc<dyn adk_core::InvocationContext>).await;
            }
        };

        Box::pin(s)
    }

    /// Convenience method that accepts string arguments.
    ///
    /// Converts `user_id` and `session_id` to their typed equivalents
    /// and delegates to [`run()`](Self::run).
    ///
    /// # Errors
    ///
    /// Returns an error if either string fails identity validation
    /// (empty, contains null bytes, or exceeds length limit).
    pub async fn run_str(
        &self,
        user_id: &str,
        session_id: &str,
        user_content: Content,
    ) -> Result<EventStream> {
        let user_id = UserId::try_from(user_id)?;
        let session_id = SessionId::try_from(session_id)?;
        self.run(user_id, session_id, user_content).await
    }

    /// Interrupts **all** in-flight invocations for the given session.
    ///
    /// Cancels every invocation-scoped token currently registered under this
    /// session id (matching the caller's per-session disconnect intent). Events
    /// already produced and appended to the session are preserved — only future
    /// events are stopped.
    ///
    /// Returns `true` if at least one running invocation was found and cancelled,
    /// `false` if no active run exists for that session ID.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Start a run in the background
    /// let mut stream = runner.run(user_id, session_id, content).await?;
    /// tokio::spawn(async move { while stream.next().await.is_some() {} });
    ///
    /// // Later, interrupt it
    /// let was_running = runner.interrupt("session-1");
    /// assert!(was_running);
    ///
    /// // Redirect with a new instruction
    /// let mut stream = runner.run(user_id, session_id, new_content).await?;
    /// ```
    pub fn interrupt(&self, session_id: &str) -> bool {
        let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let Some(inv_ids) = reg.by_session.get(session_id) else {
            tracing::debug!(session.id = session_id, "no active run to interrupt");
            return false;
        };
        let mut cancelled = 0usize;
        for inv_id in inv_ids {
            if let Some(token) = reg.active.get(inv_id) {
                token.cancel();
                cancelled += 1;
            }
        }
        if cancelled > 0 {
            tracing::info!(
                session.id = session_id,
                invocations = cancelled,
                "interrupting running agent(s)"
            );
            true
        } else {
            tracing::debug!(session.id = session_id, "no active run to interrupt");
            false
        }
    }

    /// Interrupts a single in-flight invocation by its invocation id.
    ///
    /// Cancels only the run identified by `invocation_id`, leaving any other
    /// concurrent invocations of the same session untouched. Returns `true` if the
    /// invocation was found and cancelled, `false` otherwise.
    pub fn interrupt_invocation(&self, invocation_id: &str) -> bool {
        let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = reg.active.get(invocation_id) {
            tracing::info!(invocation.id = invocation_id, "interrupting invocation");
            token.cancel();
            true
        } else {
            tracing::debug!(invocation.id = invocation_id, "no active invocation to interrupt");
            false
        }
    }

    /// Returns the session IDs of all currently running agent executions.
    pub fn active_session_ids(&self) -> Vec<String> {
        let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.by_session.keys().cloned().collect()
    }

    /// Returns a reference to the context compaction configuration, if set.
    ///
    /// This is used by the runner's generate_content loop to detect token limit
    /// errors and apply compaction strategies before retrying.
    #[cfg(feature = "context-compaction")]
    pub fn context_compaction(&self) -> Option<&crate::compaction::CompactionConfig> {
        self.context_compaction.as_deref()
    }

    /// Find which agent should handle the request based on session history
    pub fn find_agent_to_run(
        root_agent: &Arc<dyn Agent>,
        session: &dyn adk_session::Session,
    ) -> Arc<dyn Agent> {
        // Look at recent events to find last agent that responded
        let events = session.events();
        for i in (0..events.len()).rev() {
            if let Some(event) = events.at(i) {
                // Check for explicit transfer
                if let Some(target_name) = &event.actions.transfer_to_agent
                    && let Some(agent) = Self::find_agent(root_agent, target_name)
                {
                    return agent;
                }

                if event.author == "user" {
                    continue;
                }

                // Try to find this agent in the tree
                if let Some(agent) = Self::find_agent(root_agent, &event.author) {
                    // Check if agent allows transfer up the tree
                    if Self::is_transferable(root_agent, &agent) {
                        return agent;
                    }
                }
            }
        }

        // Default to root agent
        root_agent.clone()
    }

    /// Check if an agent found in session history can be resumed for the next
    /// user message.
    ///
    /// This always returns `true` because the transfer-policy enforcement
    /// (`disallow_transfer_to_parent` / `disallow_transfer_to_peers`) is
    /// handled inside `LlmAgent::run()` when it builds the `transfer_to_agent`
    /// tool's valid-target list. The runner does not need to duplicate that
    /// check here — it only needs to know whether the agent is a valid
    /// resumption target, which it always is if it exists in the tree.
    fn is_transferable(_root_agent: &Arc<dyn Agent>, _agent: &Arc<dyn Agent>) -> bool {
        true
    }

    /// Recursively search agent tree for agent with given name
    pub fn find_agent(current: &Arc<dyn Agent>, target_name: &str) -> Option<Arc<dyn Agent>> {
        if current.name() == target_name {
            return Some(current.clone());
        }

        for sub_agent in current.sub_agents() {
            if let Some(found) = Self::find_agent(sub_agent, target_name) {
                return Some(found);
            }
        }

        None
    }

    /// Compute the parent name and peer names for a given agent in the tree.
    /// Returns `(parent_name, peer_names)`.
    ///
    /// Walks the agent tree to find the parent of `target_name`, then collects
    /// the parent's name and the sibling agent names (excluding the target itself).
    pub fn compute_transfer_context(
        root: &Arc<dyn Agent>,
        target_name: &str,
    ) -> (Option<String>, Vec<String>) {
        // If the target is the root itself, there's no parent or peers
        if root.name() == target_name {
            return (None, Vec::new());
        }

        // BFS/DFS to find the parent of target_name
        fn find_parent(current: &Arc<dyn Agent>, target: &str) -> Option<Arc<dyn Agent>> {
            for sub in current.sub_agents() {
                if sub.name() == target {
                    return Some(current.clone());
                }
                if let Some(found) = find_parent(sub, target) {
                    return Some(found);
                }
            }
            None
        }

        match find_parent(root, target_name) {
            Some(parent) => {
                let parent_name = parent.name().to_string();
                let peers: Vec<String> = parent
                    .sub_agents()
                    .iter()
                    .filter(|a| a.name() != target_name)
                    .map(|a| a.name().to_string())
                    .collect();
                (Some(parent_name), peers)
            }
            None => (None, Vec::new()),
        }
    }
}
