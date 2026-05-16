//! Content event emitter for opt-in prompt/completion capture.
//!
//! Emits span events containing prompt or completion text when
//! [`SemconvConfig::capture_content`](crate::config::SemconvConfig::capture_content)
//! is enabled.

use tracing::Span;

use crate::config::SemconvConfig;

/// Emits opt-in content events on the current span.
///
/// Only emits when [`SemconvConfig::capture_content`] is `true`.
/// When content capture is disabled (the default), these methods are no-ops.
///
/// # Example
/// ```
/// use adk_telemetry::config::SemconvConfig;
/// use adk_telemetry::events::ContentEventEmitter;
///
/// let config = SemconvConfig { capture_content: true };
/// ContentEventEmitter::emit_prompt(&config, "What is the weather?");
/// ContentEventEmitter::emit_completion(&config, "The weather is sunny.");
/// ```
pub struct ContentEventEmitter;

impl ContentEventEmitter {
    /// Emit a `gen_ai.content.prompt` span event with the prompt text.
    ///
    /// Only emits when `config.capture_content` is `true`.
    pub fn emit_prompt(config: &SemconvConfig, prompt: &str) {
        if !config.capture_content {
            return;
        }
        let span = Span::current();
        if span.is_none() {
            tracing::debug!("content event emitter: no active span for prompt event");
            return;
        }
        span.in_scope(|| {
            tracing::info!(event_name = "gen_ai.content.prompt", content = prompt);
        });
    }

    /// Emit a `gen_ai.content.completion` span event with the completion text.
    ///
    /// Only emits when `config.capture_content` is `true`.
    pub fn emit_completion(config: &SemconvConfig, completion: &str) {
        if !config.capture_content {
            return;
        }
        let span = Span::current();
        if span.is_none() {
            tracing::debug!("content event emitter: no active span for completion event");
            return;
        }
        span.in_scope(|| {
            tracing::info!(event_name = "gen_ai.content.completion", content = completion);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_prompt_disabled_is_noop() {
        let config = SemconvConfig::default();
        // Should not panic even without an active span
        ContentEventEmitter::emit_prompt(&config, "test prompt");
    }

    #[test]
    fn test_emit_completion_disabled_is_noop() {
        let config = SemconvConfig::default();
        // Should not panic even without an active span
        ContentEventEmitter::emit_completion(&config, "test completion");
    }
}
