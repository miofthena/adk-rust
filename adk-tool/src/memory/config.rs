//! Configuration for memory tools.
//!
//! Provides [`MemoryToolConfig`] and its builder for controlling result limits,
//! relevance thresholds, and project scoping.

use adk_core::AdkError;

/// Shared configuration for memory tools.
///
/// Controls the maximum number of results, minimum relevance score threshold,
/// and optional project scoping for memory searches.
///
/// # Example
///
/// ```rust
/// use adk_tool::memory::MemoryToolConfig;
///
/// let config = MemoryToolConfig::builder()
///     .max_results(10)
///     .min_relevance_score(0.5)
///     .project_id("my-project")
///     .build()
///     .unwrap();
///
/// assert_eq!(config.max_results, 10);
/// assert_eq!(config.min_relevance_score, Some(0.5));
/// ```
#[derive(Debug, Clone)]
pub struct MemoryToolConfig {
    /// Maximum number of results to return. Range: 1–100. Default: 5.
    pub max_results: usize,
    /// Minimum relevance score threshold. Range: 0.0–1.0. Default: None (no threshold).
    pub min_relevance_score: Option<f32>,
    /// Optional project identifier for scoped searches. Default: None (global only).
    pub project_id: Option<String>,
}

impl Default for MemoryToolConfig {
    fn default() -> Self {
        Self { max_results: 5, min_relevance_score: None, project_id: None }
    }
}

impl MemoryToolConfig {
    /// Create a new builder for `MemoryToolConfig`.
    pub fn builder() -> MemoryToolConfigBuilder {
        MemoryToolConfigBuilder::default()
    }

    /// Validate the configuration values.
    pub(crate) fn validate(&self) -> adk_core::Result<()> {
        if self.max_results < 1 || self.max_results > 100 {
            return Err(AdkError::tool("max_results must be between 1 and 100"));
        }
        if let Some(score) = self.min_relevance_score {
            if !(0.0..=1.0).contains(&score) {
                return Err(AdkError::tool("min_relevance_score must be between 0.0 and 1.0"));
            }
        }
        Ok(())
    }
}

/// Builder for [`MemoryToolConfig`].
#[derive(Debug, Clone, Default)]
pub struct MemoryToolConfigBuilder {
    config: MemoryToolConfig,
}

impl MemoryToolConfigBuilder {
    /// Set the maximum number of results to return. Must be between 1 and 100.
    pub fn max_results(mut self, max: usize) -> Self {
        self.config.max_results = max;
        self
    }

    /// Set the minimum relevance score threshold. Must be between 0.0 and 1.0.
    pub fn min_relevance_score(mut self, score: f32) -> Self {
        self.config.min_relevance_score = Some(score);
        self
    }

    /// Set the project identifier for scoped searches.
    pub fn project_id(mut self, id: impl Into<String>) -> Self {
        self.config.project_id = Some(id.into());
        self
    }

    /// Build the configuration, validating all values.
    ///
    /// # Errors
    ///
    /// Returns an error if `max_results` is outside [1, 100] or
    /// `min_relevance_score` is outside [0.0, 1.0].
    pub fn build(self) -> adk_core::Result<MemoryToolConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MemoryToolConfig::default();
        assert_eq!(config.max_results, 5);
        assert_eq!(config.min_relevance_score, None);
        assert_eq!(config.project_id, None);
    }

    #[test]
    fn test_builder_valid() {
        let config = MemoryToolConfig::builder()
            .max_results(10)
            .min_relevance_score(0.5)
            .project_id("test-project")
            .build()
            .unwrap();

        assert_eq!(config.max_results, 10);
        assert_eq!(config.min_relevance_score, Some(0.5));
        assert_eq!(config.project_id, Some("test-project".to_string()));
    }

    #[test]
    fn test_builder_max_results_too_low() {
        let result = MemoryToolConfig::builder().max_results(0).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_max_results_too_high() {
        let result = MemoryToolConfig::builder().max_results(101).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_min_score_too_low() {
        let result = MemoryToolConfig::builder().min_relevance_score(-0.1).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_min_score_too_high() {
        let result = MemoryToolConfig::builder().min_relevance_score(1.1).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_boundary_values() {
        // min boundary
        let config =
            MemoryToolConfig::builder().max_results(1).min_relevance_score(0.0).build().unwrap();
        assert_eq!(config.max_results, 1);
        assert_eq!(config.min_relevance_score, Some(0.0));

        // max boundary
        let config =
            MemoryToolConfig::builder().max_results(100).min_relevance_score(1.0).build().unwrap();
        assert_eq!(config.max_results, 100);
        assert_eq!(config.min_relevance_score, Some(1.0));
    }
}
