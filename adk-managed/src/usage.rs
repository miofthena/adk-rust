//! Uniform usage reporting across all providers.
//!
//! This module provides the [`UsageReport`] type that normalizes token usage
//! from all LLM providers into a consistent format. After each turn, the
//! session loop extracts `input_tokens` and `output_tokens` from the
//! [`UsageMetadata`] in the LLM response and produces
//! a uniform `UsageReport`.
//!
//! # Provider Parity (Requirement 5.3)
//!
//! > Streaming token output and usage metadata (input/output tokens) SHALL be
//! > reported uniformly so the platform can meter cost per provider.
//!
//! Different providers report usage with different field names:
//! - Gemini: `prompt_token_count` / `candidates_token_count`
//! - OpenAI: `prompt_tokens` / `completion_tokens`
//! - Anthropic: `input_tokens` / `output_tokens`
//!
//! All of these are normalized into `adk-core`'s [`UsageMetadata`] by each
//! provider's client. This module further simplifies into `UsageReport` for
//! the managed runtime's uniform reporting.
//!
//! # Integration
//!
//! The session loop calls [`UsageReport::from_usage_metadata`] after each turn
//! to extract usage information. The platform can then use this for billing,
//! monitoring, and cost tracking.

use adk_core::UsageMetadata;
use serde::{Deserialize, Serialize};

/// Uniform usage report emitted after each turn.
///
/// Normalizes provider-specific token counts into a simple, consistent
/// structure that the platform uses for metering and billing.
///
/// # Example
///
/// ```rust
/// use adk_managed::usage::UsageReport;
///
/// let report = UsageReport::new(100, 50);
/// assert_eq!(report.input_tokens, 100);
/// assert_eq!(report.output_tokens, 50);
/// assert_eq!(report.total_tokens, 150);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    /// Number of tokens in the input/prompt.
    pub input_tokens: u64,
    /// Number of tokens generated in the output/response.
    pub output_tokens: u64,
    /// Total tokens (input + output). Always equals `input_tokens + output_tokens`.
    pub total_tokens: u64,
    /// Tokens consumed by thinking/reasoning (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u64>,
    /// Tokens read from cache (if provider supports caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to cache (if provider supports caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl UsageReport {
    /// Create a new usage report with the given input and output token counts.
    ///
    /// Total is computed automatically as `input_tokens + output_tokens`.
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            thinking_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    }

    /// Create a `UsageReport` from `adk-core`'s [`UsageMetadata`].
    ///
    /// This is the primary conversion used by the session loop after each turn.
    /// It normalizes the provider-specific field names into the uniform format.
    ///
    /// # Arguments
    ///
    /// * `metadata` - The raw usage metadata from the LLM response.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_core::UsageMetadata;
    /// use adk_managed::usage::UsageReport;
    ///
    /// let metadata = UsageMetadata {
    ///     prompt_token_count: 150,
    ///     candidates_token_count: 75,
    ///     total_token_count: 225,
    ///     ..Default::default()
    /// };
    ///
    /// let report = UsageReport::from_usage_metadata(&metadata);
    /// assert_eq!(report.input_tokens, 150);
    /// assert_eq!(report.output_tokens, 75);
    /// assert_eq!(report.total_tokens, 225);
    /// ```
    pub fn from_usage_metadata(metadata: &UsageMetadata) -> Self {
        let input_tokens = metadata.prompt_token_count.max(0) as u64;
        let output_tokens = metadata.candidates_token_count.max(0) as u64;
        let total_tokens = metadata.total_token_count.max(0) as u64;

        // Use the metadata's total if it's provided and non-zero,
        // otherwise compute it ourselves.
        let total = if total_tokens > 0 { total_tokens } else { input_tokens + output_tokens };

        let thinking_tokens =
            metadata.thinking_token_count.and_then(|t| if t > 0 { Some(t as u64) } else { None });

        let cache_read_tokens = metadata
            .cache_read_input_token_count
            .and_then(|t| if t > 0 { Some(t as u64) } else { None });

        let cache_write_tokens = metadata
            .cache_creation_input_token_count
            .and_then(|t| if t > 0 { Some(t as u64) } else { None });

        Self {
            input_tokens,
            output_tokens,
            total_tokens: total,
            thinking_tokens,
            cache_read_tokens,
            cache_write_tokens,
        }
    }

    /// Accumulate another report into this one (for multi-turn aggregation).
    ///
    /// This is useful for tracking total usage across an entire session.
    pub fn accumulate(&mut self, other: &UsageReport) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;

        // Accumulate optional fields
        match (self.thinking_tokens, other.thinking_tokens) {
            (Some(a), Some(b)) => self.thinking_tokens = Some(a + b),
            (None, Some(b)) => self.thinking_tokens = Some(b),
            _ => {}
        }
        match (self.cache_read_tokens, other.cache_read_tokens) {
            (Some(a), Some(b)) => self.cache_read_tokens = Some(a + b),
            (None, Some(b)) => self.cache_read_tokens = Some(b),
            _ => {}
        }
        match (self.cache_write_tokens, other.cache_write_tokens) {
            (Some(a), Some(b)) => self.cache_write_tokens = Some(a + b),
            (None, Some(b)) => self.cache_write_tokens = Some(b),
            _ => {}
        }
    }

    /// Returns true if this report has zero usage (no tokens consumed).
    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

/// Accumulated usage tracking for an entire session.
///
/// The session loop maintains one of these and calls `record_turn` after
/// each turn completes. The platform can read the cumulative usage at any time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsageTracker {
    /// Cumulative usage across all turns.
    pub cumulative: UsageReport,
    /// Number of turns completed.
    pub turn_count: u64,
    /// Usage from the most recent turn (for per-turn billing).
    pub last_turn: Option<UsageReport>,
}

impl SessionUsageTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record usage from a completed turn.
    pub fn record_turn(&mut self, turn_usage: UsageReport) {
        self.cumulative.accumulate(&turn_usage);
        self.turn_count += 1;
        self.last_turn = Some(turn_usage);
    }

    /// Get the cumulative usage report.
    pub fn total(&self) -> &UsageReport {
        &self.cumulative
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_report_new() {
        let report = UsageReport::new(100, 50);
        assert_eq!(report.input_tokens, 100);
        assert_eq!(report.output_tokens, 50);
        assert_eq!(report.total_tokens, 150);
        assert_eq!(report.thinking_tokens, None);
        assert_eq!(report.cache_read_tokens, None);
        assert_eq!(report.cache_write_tokens, None);
    }

    #[test]
    fn test_usage_report_default_is_zero() {
        let report = UsageReport::default();
        assert_eq!(report.input_tokens, 0);
        assert_eq!(report.output_tokens, 0);
        assert_eq!(report.total_tokens, 0);
        assert!(report.is_empty());
    }

    #[test]
    fn test_from_usage_metadata_basic() {
        let metadata = UsageMetadata {
            prompt_token_count: 200,
            candidates_token_count: 100,
            total_token_count: 300,
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.input_tokens, 200);
        assert_eq!(report.output_tokens, 100);
        assert_eq!(report.total_tokens, 300);
    }

    #[test]
    fn test_from_usage_metadata_with_thinking_tokens() {
        let metadata = UsageMetadata {
            prompt_token_count: 150,
            candidates_token_count: 80,
            total_token_count: 230,
            thinking_token_count: Some(50),
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.input_tokens, 150);
        assert_eq!(report.output_tokens, 80);
        assert_eq!(report.total_tokens, 230);
        assert_eq!(report.thinking_tokens, Some(50));
    }

    #[test]
    fn test_from_usage_metadata_with_cache_tokens() {
        let metadata = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            cache_read_input_token_count: Some(30),
            cache_creation_input_token_count: Some(10),
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.cache_read_tokens, Some(30));
        assert_eq!(report.cache_write_tokens, Some(10));
    }

    #[test]
    fn test_from_usage_metadata_zero_total_computes_automatically() {
        let metadata = UsageMetadata {
            prompt_token_count: 80,
            candidates_token_count: 40,
            total_token_count: 0, // Provider didn't report total
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.input_tokens, 80);
        assert_eq!(report.output_tokens, 40);
        assert_eq!(report.total_tokens, 120); // Computed: 80 + 40
    }

    #[test]
    fn test_from_usage_metadata_negative_values_clamped_to_zero() {
        let metadata = UsageMetadata {
            prompt_token_count: -5,
            candidates_token_count: -10,
            total_token_count: -15,
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.input_tokens, 0);
        assert_eq!(report.output_tokens, 0);
        assert_eq!(report.total_tokens, 0);
    }

    #[test]
    fn test_from_usage_metadata_zero_thinking_not_reported() {
        let metadata = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            thinking_token_count: Some(0),
            ..Default::default()
        };

        let report = UsageReport::from_usage_metadata(&metadata);
        assert_eq!(report.thinking_tokens, None); // Zero thinking is not reported
    }

    #[test]
    fn test_accumulate() {
        let mut total = UsageReport::new(100, 50);
        let turn2 = UsageReport::new(80, 40);

        total.accumulate(&turn2);

        assert_eq!(total.input_tokens, 180);
        assert_eq!(total.output_tokens, 90);
        assert_eq!(total.total_tokens, 270);
    }

    #[test]
    fn test_accumulate_with_optional_fields() {
        let mut total = UsageReport {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            thinking_tokens: Some(20),
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        let turn2 = UsageReport {
            input_tokens: 80,
            output_tokens: 40,
            total_tokens: 120,
            thinking_tokens: Some(15),
            cache_read_tokens: Some(10),
            cache_write_tokens: None,
        };

        total.accumulate(&turn2);

        assert_eq!(total.thinking_tokens, Some(35));
        assert_eq!(total.cache_read_tokens, Some(10));
        assert_eq!(total.cache_write_tokens, None);
    }

    #[test]
    fn test_is_empty() {
        assert!(UsageReport::default().is_empty());
        assert!(UsageReport::new(0, 0).is_empty());
        assert!(!UsageReport::new(1, 0).is_empty());
        assert!(!UsageReport::new(0, 1).is_empty());
    }

    #[test]
    fn test_session_usage_tracker_record_turn() {
        let mut tracker = SessionUsageTracker::new();
        assert_eq!(tracker.turn_count, 0);
        assert!(tracker.last_turn.is_none());

        tracker.record_turn(UsageReport::new(100, 50));
        assert_eq!(tracker.turn_count, 1);
        assert_eq!(tracker.cumulative.input_tokens, 100);
        assert_eq!(tracker.cumulative.output_tokens, 50);
        assert_eq!(tracker.cumulative.total_tokens, 150);
        assert_eq!(tracker.last_turn, Some(UsageReport::new(100, 50)));

        tracker.record_turn(UsageReport::new(80, 40));
        assert_eq!(tracker.turn_count, 2);
        assert_eq!(tracker.cumulative.input_tokens, 180);
        assert_eq!(tracker.cumulative.output_tokens, 90);
        assert_eq!(tracker.cumulative.total_tokens, 270);
        assert_eq!(tracker.last_turn, Some(UsageReport::new(80, 40)));
    }

    #[test]
    fn test_usage_report_serialization_round_trip() {
        let report = UsageReport {
            input_tokens: 150,
            output_tokens: 75,
            total_tokens: 225,
            thinking_tokens: Some(30),
            cache_read_tokens: Some(20),
            cache_write_tokens: None,
        };

        let json = serde_json::to_string(&report).unwrap();
        let deserialized: UsageReport = serde_json::from_str(&json).unwrap();

        assert_eq!(report, deserialized);
    }

    #[test]
    fn test_usage_report_serialization_omits_none_fields() {
        let report = UsageReport::new(100, 50);
        let value = serde_json::to_value(&report).unwrap();

        // Optional None fields should not appear in JSON
        assert!(value.get("thinking_tokens").is_none());
        assert!(value.get("cache_read_tokens").is_none());
        assert!(value.get("cache_write_tokens").is_none());

        // Required fields must appear
        assert_eq!(value["input_tokens"], 100);
        assert_eq!(value["output_tokens"], 50);
        assert_eq!(value["total_tokens"], 150);
    }

    #[test]
    fn test_uniform_reporting_across_providers() {
        // Simulate usage from different providers all going through UsageMetadata.
        // The key guarantee: regardless of provider, the UsageReport looks the same.

        // Gemini response
        let gemini_meta = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            ..Default::default()
        };

        // OpenAI response (same tokens, different internal naming)
        let openai_meta = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            ..Default::default()
        };

        // Anthropic response (same tokens)
        let anthropic_meta = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            ..Default::default()
        };

        let gemini_report = UsageReport::from_usage_metadata(&gemini_meta);
        let openai_report = UsageReport::from_usage_metadata(&openai_meta);
        let anthropic_report = UsageReport::from_usage_metadata(&anthropic_meta);

        // All reports should be identical
        assert_eq!(gemini_report, openai_report);
        assert_eq!(openai_report, anthropic_report);

        // And serialization should be byte-identical
        let json1 = serde_json::to_string(&gemini_report).unwrap();
        let json2 = serde_json::to_string(&openai_report).unwrap();
        let json3 = serde_json::to_string(&anthropic_report).unwrap();
        assert_eq!(json1, json2);
        assert_eq!(json2, json3);
    }
}
