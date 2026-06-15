//! # Transcript Aggregator
//!
//! State machine that collects streaming deltas (`TextDelta`, `TranscriptDelta`)
//! into complete turns, emitting `AggregatedEvent` values when a turn finalizes
//! (either via `ResponseDone` or interruption by a new `ResponseCreated`).

use crate::events::ServerEvent;

use super::{AggregatedEvent, CompletedToolCall};

/// Internal accumulator for an in-progress assistant turn.
///
/// Collects text deltas, audio transcript deltas, and tool calls
/// until the turn is finalized (either normally or via interruption).
struct TurnAccumulator {
    /// Provider item ID for delta attribution.
    item_id: String,
    /// Accumulated text output from `TextDelta` events.
    text: String,
    /// Accumulated audio transcript from `TranscriptDelta` events.
    audio_transcript: String,
    /// Tool calls executed during this turn.
    tool_calls: Vec<CompletedToolCall>,
}

impl TurnAccumulator {
    /// Creates a new empty accumulator.
    fn new() -> Self {
        Self {
            item_id: String::new(),
            text: String::new(),
            audio_transcript: String::new(),
            tool_calls: Vec::new(),
        }
    }
}

/// Collects streaming deltas into complete turns.
///
/// The `TranscriptAggregator` is a state machine that processes `ServerEvent`
/// values and emits `AggregatedEvent` values when a turn completes. It handles:
///
/// - Starting a new turn on `ResponseCreated`
/// - Accumulating text via `TextDelta`
/// - Accumulating audio transcript via `TranscriptDelta`
/// - Finalizing a turn on `ResponseDone`
/// - Handling interrupted turns (new `ResponseCreated` during accumulation)
///
/// # Example
///
/// ```rust,ignore
/// let mut aggregator = TranscriptAggregator::new();
///
/// for event in server_events {
///     if let Some(aggregated) = aggregator.process(&event) {
///         // Handle the completed turn
///     }
/// }
/// ```
pub struct TranscriptAggregator {
    /// Current accumulating turn (None if idle between turns).
    current_turn: Option<TurnAccumulator>,
}

impl TranscriptAggregator {
    /// Creates a new `TranscriptAggregator` in idle state.
    pub fn new() -> Self {
        Self { current_turn: None }
    }

    /// Process a `ServerEvent` and optionally emit an `AggregatedEvent`.
    ///
    /// Returns `Some(AggregatedEvent)` when a turn completes (either normally
    /// via `ResponseDone` or via interruption by a new `ResponseCreated`).
    /// Returns `None` for intermediate events that only accumulate state.
    pub fn process(&mut self, event: &ServerEvent) -> Option<AggregatedEvent> {
        match event {
            ServerEvent::ResponseCreated { .. } => {
                // Finalize previous turn as interrupted if one was in progress
                let interrupted = self.finalize_current(true);
                // Start accumulating a new turn
                self.current_turn = Some(TurnAccumulator::new());
                interrupted
            }
            ServerEvent::TextDelta { delta, item_id, .. } => {
                if let Some(ref mut turn) = self.current_turn {
                    turn.item_id = item_id.clone();
                    turn.text.push_str(delta);
                }
                None
            }
            ServerEvent::TranscriptDelta { delta, item_id, .. } => {
                if let Some(ref mut turn) = self.current_turn {
                    turn.item_id = item_id.clone();
                    turn.audio_transcript.push_str(delta);
                }
                None
            }
            ServerEvent::ResponseDone { .. } => self.finalize_current(false),
            _ => None,
        }
    }

    /// Finalize the current turn accumulator and emit a `TurnComplete` event.
    ///
    /// Returns `None` if no turn was in progress.
    fn finalize_current(&mut self, interrupted: bool) -> Option<AggregatedEvent> {
        self.current_turn.take().map(|turn| AggregatedEvent::TurnComplete {
            text: turn.text,
            audio_transcript: turn.audio_transcript,
            tool_calls: turn.tool_calls,
            item_id: turn.item_id,
            interrupted,
        })
    }

    /// Record a completed tool call in the current turn.
    ///
    /// If no turn is currently in progress, the call is silently dropped.
    pub fn record_tool_call(&mut self, call: CompletedToolCall) {
        if let Some(ref mut turn) = self.current_turn {
            turn.tool_calls.push(call);
        }
    }

    /// Process a finalized user transcript from speech recognition.
    ///
    /// Returns a `UserUtteranceComplete` event containing the full transcript.
    pub fn process_user_transcript(&mut self, transcript: &str) -> AggregatedEvent {
        AggregatedEvent::UserUtteranceComplete { transcript: transcript.to_string() }
    }
}

impl Default for TranscriptAggregator {
    fn default() -> Self {
        Self::new()
    }
}
