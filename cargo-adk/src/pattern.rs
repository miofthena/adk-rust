//! Enterprise pattern definitions for the composable scaffolding engine.
//!
//! Patterns are pre-composed combinations of a base template plus addons,
//! targeting production use cases.

use crate::template::AgentCodeFragments;

/// A pre-composed combination of template + addons for production use cases.
#[derive(Debug, Clone)]
pub struct EnterprisePattern {
    /// Pattern name used in CLI (e.g., "production", "chatbot").
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// The base agent template this pattern builds on.
    pub base_template: &'static str,
    /// Addons automatically included in this pattern.
    pub included_addons: Vec<&'static str>,
    /// Override the default feature set (if `Some`, replaces union logic).
    pub override_features: Option<Vec<&'static str>>,
    /// Optional code fragments that override or extend the base template.
    pub code_fragments: Option<AgentCodeFragments>,
}
