//! Property-based test for ToolBridgeAdapter round-trip.
//!
//! **Feature: realtime-adk-integration, Property 1: Tool Bridge Round-Trip**
//! *For any* deterministic `Arc<dyn Tool>`, wrapping in `ToolBridgeAdapter` and calling
//! `execute(ToolCall { arguments })` SHALL produce the same `Value` as calling
//! `tool.execute(ctx, arguments)` directly.
//! **Validates: Requirements 1.1, 1.2, 1.3**

#![cfg(feature = "integration")]

use std::sync::Arc;

use adk_core::{
    Artifacts, CallbackContext, Content, EventActions, MemoryEntry, ReadonlyContext, Tool,
    ToolContext,
};
use adk_realtime::events::ToolCall;
use adk_realtime::integration::{DefaultToolContextFactory, SessionIdentity, ToolBridgeAdapter};
use adk_realtime::runner::ToolHandler;
use async_trait::async_trait;
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::Mutex;

// ─── Deterministic Echo Tool ─────────────────────────────────────────────────

/// A deterministic test tool that echoes its arguments back as the result.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes arguments back unchanged"
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> adk_core::Result<Value> {
        Ok(args)
    }
}

/// A deterministic test tool that wraps the arguments in an envelope.
struct EnvelopeTool;

#[async_trait]
impl Tool for EnvelopeTool {
    fn name(&self) -> &str {
        "envelope"
    }

    fn description(&self) -> &str {
        "Wraps arguments in an envelope"
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> adk_core::Result<Value> {
        Ok(json!({ "result": args, "status": "ok" }))
    }
}

// ─── Test ToolContext ────────────────────────────────────────────────────────

/// Minimal ToolContext for direct tool execution in tests.
struct TestToolContext {
    function_call_id: String,
    content: Content,
    actions: Mutex<EventActions>,
}

impl TestToolContext {
    fn new(function_call_id: &str) -> Self {
        Self {
            function_call_id: function_call_id.to_string(),
            content: Content::new("user"),
            actions: Mutex::new(EventActions::default()),
        }
    }
}

#[async_trait]
impl ReadonlyContext for TestToolContext {
    fn invocation_id(&self) -> &str {
        &self.function_call_id
    }
    fn agent_name(&self) -> &str {
        "realtime"
    }
    fn user_id(&self) -> &str {
        "test-user"
    }
    fn app_name(&self) -> &str {
        "test-app"
    }
    fn session_id(&self) -> &str {
        "test-session"
    }
    fn branch(&self) -> &str {
        "main"
    }
    fn user_content(&self) -> &Content {
        &self.content
    }
}

#[async_trait]
impl CallbackContext for TestToolContext {
    fn artifacts(&self) -> Option<Arc<dyn Artifacts>> {
        None
    }
}

#[async_trait]
impl ToolContext for TestToolContext {
    fn function_call_id(&self) -> &str {
        &self.function_call_id
    }
    fn actions(&self) -> EventActions {
        self.actions.lock().unwrap().clone()
    }
    fn set_actions(&self, actions: EventActions) {
        *self.actions.lock().unwrap() = actions;
    }
    async fn search_memory(&self, _query: &str) -> adk_core::Result<Vec<MemoryEntry>> {
        Ok(vec![])
    }
}

// ─── Generators ──────────────────────────────────────────────────────────────

/// Strategy for generating arbitrary JSON values for tool arguments.
fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>().prop_filter("must be finite", |f| f.is_finite()).prop_map(|f| json!(f)),
        "[a-zA-Z0-9_ ]{0,50}".prop_map(Value::String),
    ];

    leaf.prop_recursive(
        3,  // max depth
        64, // max nodes
        10, // items per collection
        |inner| {
            prop_oneof![
                // JSON array
                prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
                // JSON object
                prop::collection::vec(
                    ("[a-zA-Z_][a-zA-Z0-9_]{0,10}".prop_map(String::from), inner),
                    0..5
                )
                .prop_map(|entries| { Value::Object(entries.into_iter().collect()) }),
            ]
        },
    )
}

/// Strategy for generating call IDs.
fn arb_call_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{5,20}".prop_map(String::from)
}

// ─── Property Test ───────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: realtime-adk-integration, Property 1: Tool Bridge Round-Trip**
    /// *For any* deterministic `Arc<dyn Tool>` (EchoTool), wrapping in `ToolBridgeAdapter`
    /// and calling `execute(ToolCall { arguments })` SHALL produce the same `Value`
    /// as calling `tool.execute(ctx, arguments)` directly.
    /// **Validates: Requirements 1.1, 1.2, 1.3**
    #[test]
    fn prop_tool_bridge_round_trip_echo(
        args in arb_json_value(),
        call_id in arb_call_id(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tool: Arc<dyn Tool> = Arc::new(EchoTool);

            // Create matching context factories
            let identity = SessionIdentity {
                app_name: "test-app".to_string(),
                user_id: "test-user".to_string(),
                session_id: "test-session".to_string(),
            };
            let context_factory = Arc::new(DefaultToolContextFactory {
                identity,
                memory_service: None,
            });

            // Create the adapter
            let adapter = ToolBridgeAdapter::new(tool.clone(), context_factory.clone());

            // Execute via adapter (ToolHandler interface)
            let tool_call = ToolCall {
                call_id: call_id.clone(),
                name: "echo".to_string(),
                arguments: args.clone(),
            };
            let adapter_result = adapter.execute(&tool_call).await.unwrap();

            // Execute directly via Tool interface
            let ctx: Arc<dyn ToolContext> = Arc::new(TestToolContext::new(&call_id));
            let direct_result = tool.execute(ctx, args).await.unwrap();

            // They must match
            assert_eq!(adapter_result, direct_result);
        });
    }

    /// **Feature: realtime-adk-integration, Property 1: Tool Bridge Round-Trip (EnvelopeTool)**
    /// *For any* deterministic `Arc<dyn Tool>` (EnvelopeTool), wrapping in `ToolBridgeAdapter`
    /// and calling `execute(ToolCall { arguments })` SHALL produce the same `Value`
    /// as calling `tool.execute(ctx, arguments)` directly.
    /// **Validates: Requirements 1.1, 1.2, 1.3**
    #[test]
    fn prop_tool_bridge_round_trip_envelope(
        args in arb_json_value(),
        call_id in arb_call_id(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tool: Arc<dyn Tool> = Arc::new(EnvelopeTool);

            let identity = SessionIdentity {
                app_name: "test-app".to_string(),
                user_id: "test-user".to_string(),
                session_id: "test-session".to_string(),
            };
            let context_factory = Arc::new(DefaultToolContextFactory {
                identity,
                memory_service: None,
            });

            let adapter = ToolBridgeAdapter::new(tool.clone(), context_factory.clone());

            let tool_call = ToolCall {
                call_id: call_id.clone(),
                name: "envelope".to_string(),
                arguments: args.clone(),
            };
            let adapter_result = adapter.execute(&tool_call).await.unwrap();

            let ctx: Arc<dyn ToolContext> = Arc::new(TestToolContext::new(&call_id));
            let direct_result = tool.execute(ctx, args).await.unwrap();

            assert_eq!(adapter_result, direct_result);
        });
    }
}
