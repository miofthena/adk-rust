//! Runtime configuration for semantic convention emission.
//!
//! Controls opt-in behaviors that cannot be gated at compile time,
//! such as content event capture.

/// Runtime configuration for semantic convention emission.
///
/// Controls opt-in behaviors like content capture that are disabled by default
/// for privacy protection.
///
/// # Example
/// ```
/// use adk_telemetry::config::SemconvConfig;
///
/// // Default: content capture disabled
/// let config = SemconvConfig::default();
/// assert!(!config.capture_content);
///
/// // Enable content capture for debugging
/// let debug_config = SemconvConfig { capture_content: true };
/// ```
#[derive(Debug, Clone, Default)]
pub struct SemconvConfig {
    /// Whether to capture prompt/completion content as span events.
    ///
    /// Default: `false` (privacy protection).
    /// When enabled, full prompt and completion text is emitted as span events.
    pub capture_content: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disables_content_capture() {
        let config = SemconvConfig::default();
        assert!(!config.capture_content);
    }

    #[test]
    fn test_config_can_enable_content_capture() {
        let config = SemconvConfig { capture_content: true };
        assert!(config.capture_content);
    }
}
