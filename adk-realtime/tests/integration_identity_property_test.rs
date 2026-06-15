//! Property test for identity propagation through `DefaultToolContextFactory`.
//!
//! **Feature: realtime-adk-integration, Property 6: Identity Propagation**
//!
//! Validates that for any `SessionIdentity`, every `ToolContext` created by
//! `DefaultToolContextFactory` reports the same `app_name`, `user_id`, and `session_id`.

#![cfg(feature = "integration")]

use adk_realtime::integration::{DefaultToolContextFactory, SessionIdentity, ToolContextFactory};
use proptest::prelude::*;

fn arb_identity() -> impl Strategy<Value = (String, String, String)> {
    (
        "[a-z][a-z0-9_-]{2,20}", // app_name
        "[a-z][a-z0-9_-]{2,20}", // user_id
        "[a-z0-9]{8,32}",        // session_id
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: realtime-adk-integration, Property 6: Identity Propagation**
    /// *For any* SessionIdentity, every ToolContext created by DefaultToolContextFactory
    /// SHALL report the same app_name, user_id, and session_id.
    /// **Validates: Requirements 1.6, 6.6**
    #[test]
    fn prop_identity_propagation((app_name, user_id, session_id) in arb_identity()) {
        let factory = DefaultToolContextFactory {
            identity: SessionIdentity {
                app_name: app_name.clone(),
                user_id: user_id.clone(),
                session_id: session_id.clone(),
            },
            memory_service: None,
        };

        // Create multiple contexts with different call IDs
        let ctx1 = factory.create_context("call-1");
        let ctx2 = factory.create_context("call-2");
        let ctx3 = factory.create_context("call-3");

        // All contexts should report the same identity
        prop_assert_eq!(ctx1.app_name(), app_name.as_str());
        prop_assert_eq!(ctx1.user_id(), user_id.as_str());
        prop_assert_eq!(ctx1.session_id(), session_id.as_str());

        prop_assert_eq!(ctx2.app_name(), app_name.as_str());
        prop_assert_eq!(ctx2.user_id(), user_id.as_str());
        prop_assert_eq!(ctx2.session_id(), session_id.as_str());

        prop_assert_eq!(ctx3.app_name(), app_name.as_str());
        prop_assert_eq!(ctx3.user_id(), user_id.as_str());
        prop_assert_eq!(ctx3.session_id(), session_id.as_str());
    }
}
