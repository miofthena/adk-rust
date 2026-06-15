//! Property tests for TranscriptAggregator concatenation correctness.
//!
//! **Feature: realtime-adk-integration, Property 2: Transcript Completeness**
//! *For any* sequence of `TextDelta` strings between `ResponseCreated` and `ResponseDone`,
//! the `TurnComplete.text` field SHALL equal the concatenation of all delta strings in order.
//! **Validates: Requirements 5.1, 5.2, 5.3**

#![cfg(feature = "integration")]

use adk_realtime::events::ServerEvent;
use adk_realtime::integration::{AggregatedEvent, TranscriptAggregator};
use proptest::prelude::*;
use serde_json::json;

/// Generate a vector of non-empty text delta strings.
/// Uses alphanumeric characters plus common punctuation and spaces.
fn arb_text_deltas() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-zA-Z0-9 .,!?]{1,50}", 1..20)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: realtime-adk-integration, Property 2: Transcript Completeness**
    /// *For any* sequence of TextDelta strings between ResponseCreated and ResponseDone,
    /// the TurnComplete.text SHALL equal the concatenation of all delta strings in order.
    /// **Validates: Requirements 5.1, 5.2, 5.3**
    #[test]
    fn prop_transcript_completeness(deltas in arb_text_deltas()) {
        let mut aggregator = TranscriptAggregator::new();

        // Start a turn with ResponseCreated
        let response_created = ServerEvent::ResponseCreated {
            event_id: "evt-1".to_string(),
            response: json!({}),
        };
        let result = aggregator.process(&response_created);
        prop_assert!(result.is_none(), "ResponseCreated should not emit a TurnComplete");

        // Feed text deltas
        for (i, delta_text) in deltas.iter().enumerate() {
            let text_delta = ServerEvent::TextDelta {
                event_id: format!("evt-td-{i}"),
                response_id: "resp-1".to_string(),
                item_id: "item-1".to_string(),
                output_index: 0,
                content_index: 0,
                delta: delta_text.clone(),
            };
            let result = aggregator.process(&text_delta);
            prop_assert!(result.is_none(), "TextDelta should not emit a TurnComplete");
        }

        // End the turn with ResponseDone
        let response_done = ServerEvent::ResponseDone {
            event_id: "evt-done".to_string(),
            response: json!({}),
        };
        let result = aggregator.process(&response_done);
        prop_assert!(result.is_some(), "ResponseDone should emit a TurnComplete");

        // Verify concatenation equals all deltas joined in order
        if let Some(AggregatedEvent::TurnComplete { text, interrupted, .. }) = result {
            let expected: String = deltas.into_iter().collect();
            prop_assert_eq!(text, expected, "TurnComplete.text must equal concatenation of all deltas");
            prop_assert!(!interrupted, "A normal ResponseDone turn should not be marked interrupted");
        } else {
            prop_assert!(false, "Expected TurnComplete event from ResponseDone");
        }
    }

    /// **Feature: realtime-adk-integration, Property 2: Transcript Completeness**
    /// *For any* single TextDelta, the TurnComplete.text SHALL equal that delta exactly.
    /// **Validates: Requirements 5.1, 5.2**
    #[test]
    fn prop_single_delta_completeness(delta_text in "[a-zA-Z0-9 .,!?]{1,100}") {
        let mut aggregator = TranscriptAggregator::new();

        aggregator.process(&ServerEvent::ResponseCreated {
            event_id: "evt-1".to_string(),
            response: json!({}),
        });

        aggregator.process(&ServerEvent::TextDelta {
            event_id: "evt-td-0".to_string(),
            response_id: "resp-1".to_string(),
            item_id: "item-1".to_string(),
            output_index: 0,
            content_index: 0,
            delta: delta_text.clone(),
        });

        let result = aggregator.process(&ServerEvent::ResponseDone {
            event_id: "evt-done".to_string(),
            response: json!({}),
        });

        prop_assert!(result.is_some());
        if let Some(AggregatedEvent::TurnComplete { text, .. }) = result {
            prop_assert_eq!(text, delta_text, "Single delta must round-trip exactly");
        } else {
            prop_assert!(false, "Expected TurnComplete");
        }
    }

    /// **Feature: realtime-adk-integration, Property 2: Transcript Completeness**
    /// *For any* empty sequence of deltas (ResponseCreated immediately followed by ResponseDone),
    /// the TurnComplete.text SHALL be an empty string.
    /// **Validates: Requirements 5.1, 5.3**
    #[test]
    fn prop_empty_turn_produces_empty_text(_seed in 0u32..100u32) {
        let mut aggregator = TranscriptAggregator::new();

        aggregator.process(&ServerEvent::ResponseCreated {
            event_id: "evt-1".to_string(),
            response: json!({}),
        });

        let result = aggregator.process(&ServerEvent::ResponseDone {
            event_id: "evt-done".to_string(),
            response: json!({}),
        });

        prop_assert!(result.is_some(), "ResponseDone should emit TurnComplete even with no deltas");
        if let Some(AggregatedEvent::TurnComplete { text, interrupted, .. }) = result {
            prop_assert_eq!(text, String::new(), "Empty turn should produce empty text");
            prop_assert!(!interrupted, "Empty turn should not be interrupted");
        } else {
            prop_assert!(false, "Expected TurnComplete");
        }
    }
}
