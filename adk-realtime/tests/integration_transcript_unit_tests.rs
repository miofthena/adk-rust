//! Unit tests for `TranscriptAggregator`.
//!
//! Tests cover:
//! - Interrupted turn finalization (new `ResponseCreated` during accumulation)
//! - `item_id` attribution across multiple overlapping responses
//! - `record_tool_call` association with current turn
//! - Empty turn (ResponseCreated immediately followed by ResponseDone)
//! - `process_user_transcript` returns the correct event
//!
//! Requirements: 5.5, 5.6

#![cfg(feature = "integration")]

use adk_realtime::events::ServerEvent;
use adk_realtime::integration::{AggregatedEvent, CompletedToolCall, TranscriptAggregator};
use serde_json::json;

/// Helper to create a `ResponseCreated` event.
fn response_created() -> ServerEvent {
    ServerEvent::ResponseCreated { event_id: "evt_1".to_string(), response: json!({}) }
}

/// Helper to create a `ResponseDone` event.
fn response_done() -> ServerEvent {
    ServerEvent::ResponseDone { event_id: "evt_done".to_string(), response: json!({}) }
}

/// Helper to create a `TextDelta` event.
fn text_delta(delta: &str, item_id: &str) -> ServerEvent {
    ServerEvent::TextDelta {
        event_id: "evt_td".to_string(),
        response_id: "resp_1".to_string(),
        item_id: item_id.to_string(),
        output_index: 0,
        content_index: 0,
        delta: delta.to_string(),
    }
}

/// Helper to create a `TranscriptDelta` event.
fn transcript_delta(delta: &str, item_id: &str) -> ServerEvent {
    ServerEvent::TranscriptDelta {
        event_id: "evt_trd".to_string(),
        response_id: "resp_1".to_string(),
        item_id: item_id.to_string(),
        output_index: 0,
        content_index: 0,
        delta: delta.to_string(),
    }
}

/// Test 1: Interrupted turn finalization.
///
/// When a new `ResponseCreated` arrives while a turn is being accumulated,
/// the previous turn should be finalized with `interrupted: true`.
#[test]
fn test_interrupted_turn_finalization() {
    let mut agg = TranscriptAggregator::new();

    // Start first turn
    let result = agg.process(&response_created());
    assert!(result.is_none(), "No prior turn to finalize");

    // Accumulate some text
    let result = agg.process(&text_delta("hello", "item_A"));
    assert!(result.is_none());

    // Interrupt: new ResponseCreated arrives
    let result = agg.process(&response_created());
    assert!(result.is_some(), "Should finalize the interrupted turn");

    match result.unwrap() {
        AggregatedEvent::TurnComplete { text, interrupted, item_id, .. } => {
            assert_eq!(text, "hello");
            assert!(interrupted, "Turn should be marked as interrupted");
            assert_eq!(item_id, "item_A");
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}

/// Test 2: Item ID attribution across multiple deltas.
///
/// When multiple TextDelta events arrive with different item_ids,
/// the TurnComplete should have the last item_id seen.
#[test]
fn test_item_id_attribution() {
    let mut agg = TranscriptAggregator::new();

    agg.process(&response_created());
    agg.process(&text_delta("first ", "item_A"));
    agg.process(&text_delta("second", "item_B"));

    let result = agg.process(&response_done());
    assert!(result.is_some());

    match result.unwrap() {
        AggregatedEvent::TurnComplete { item_id, text, .. } => {
            assert_eq!(item_id, "item_B", "Should use the last item_id");
            assert_eq!(text, "first second");
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}

/// Test 3: Tool call recording is associated with the current turn.
///
/// A `record_tool_call` during an active turn should include the tool call
/// in the emitted `TurnComplete`.
#[test]
fn test_record_tool_call_association() {
    let mut agg = TranscriptAggregator::new();

    agg.process(&response_created());
    agg.process(&text_delta("thinking...", "item_A"));

    // Record a tool call
    agg.record_tool_call(CompletedToolCall {
        call_id: "call_123".to_string(),
        name: "get_weather".to_string(),
        arguments: json!({"city": "Seattle"}),
        result: json!({"temp": 55}),
    });

    let result = agg.process(&response_done());
    assert!(result.is_some());

    match result.unwrap() {
        AggregatedEvent::TurnComplete { tool_calls, .. } => {
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].call_id, "call_123");
            assert_eq!(tool_calls[0].name, "get_weather");
            assert_eq!(tool_calls[0].arguments, json!({"city": "Seattle"}));
            assert_eq!(tool_calls[0].result, json!({"temp": 55}));
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}

/// Test 4: Empty turn (ResponseCreated immediately followed by ResponseDone).
///
/// An empty turn should emit a `TurnComplete` with empty text and audio_transcript.
#[test]
fn test_empty_turn() {
    let mut agg = TranscriptAggregator::new();

    agg.process(&response_created());
    let result = agg.process(&response_done());
    assert!(result.is_some());

    match result.unwrap() {
        AggregatedEvent::TurnComplete {
            text,
            audio_transcript,
            tool_calls,
            item_id,
            interrupted,
        } => {
            assert!(text.is_empty(), "Text should be empty");
            assert!(audio_transcript.is_empty(), "Audio transcript should be empty");
            assert!(tool_calls.is_empty(), "No tool calls expected");
            assert!(item_id.is_empty(), "No item_id assigned yet");
            assert!(!interrupted, "Not interrupted — ended normally");
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}

/// Test 5: UserUtteranceComplete via `process_user_transcript`.
///
/// `process_user_transcript` should return a `UserUtteranceComplete` event
/// with the provided transcript.
#[test]
fn test_user_utterance_complete() {
    let mut agg = TranscriptAggregator::new();

    let event = agg.process_user_transcript("Hello, how are you?");

    match event {
        AggregatedEvent::UserUtteranceComplete { transcript } => {
            assert_eq!(transcript, "Hello, how are you?");
        }
        other => panic!("Expected UserUtteranceComplete, got {other:?}"),
    }
}

/// Additional coverage: tool call recorded outside of an active turn is silently dropped.
#[test]
fn test_tool_call_outside_turn_is_dropped() {
    let mut agg = TranscriptAggregator::new();

    // No turn in progress — record_tool_call should not panic
    agg.record_tool_call(CompletedToolCall {
        call_id: "orphan".to_string(),
        name: "orphan_tool".to_string(),
        arguments: json!({}),
        result: json!({}),
    });

    // Start and complete a turn — should not contain the orphaned tool call
    agg.process(&response_created());
    let result = agg.process(&response_done());
    assert!(result.is_some());

    match result.unwrap() {
        AggregatedEvent::TurnComplete { tool_calls, .. } => {
            assert!(tool_calls.is_empty(), "Orphaned tool call should not appear");
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}

/// Additional coverage: TranscriptDelta accumulates audio_transcript correctly.
#[test]
fn test_transcript_delta_accumulation() {
    let mut agg = TranscriptAggregator::new();

    agg.process(&response_created());
    agg.process(&transcript_delta("Hi ", "item_X"));
    agg.process(&transcript_delta("there", "item_X"));

    let result = agg.process(&response_done());
    assert!(result.is_some());

    match result.unwrap() {
        AggregatedEvent::TurnComplete { audio_transcript, text, .. } => {
            assert_eq!(audio_transcript, "Hi there");
            assert!(text.is_empty(), "No TextDelta was sent");
        }
        other => panic!("Expected TurnComplete, got {other:?}"),
    }
}
