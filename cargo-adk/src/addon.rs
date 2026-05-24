//! Capability addon definitions for the composable scaffolding engine.
//!
//! Addons define *cross-cutting capabilities* that can be composed with any compatible template.

use crate::template::FileFragment;

/// A dependency to add to the generated project's `Cargo.toml`.
#[derive(Debug, Clone)]
pub struct DependencySpec {
    /// Crate name on crates.io.
    pub crate_name: &'static str,
    /// Version requirement string.
    pub version: &'static str,
    /// Features to enable on this dependency.
    pub features: Vec<&'static str>,
}

/// Code fragments contributed by an addon.
#[derive(Debug, Clone)]
pub struct AddonCodeFragments {
    /// Import statements to add to `main.rs`.
    pub imports: Vec<&'static str>,
    /// Initialization code (e.g., tracing subscriber setup).
    pub initialization: &'static str,
    /// Agent builder method calls (e.g., `.tool(...)`, `.session(...)`).
    pub agent_builder_calls: &'static str,
    /// Environment variables to document in `.env.example`.
    pub env_vars: Vec<(&'static str, &'static str)>,
    /// Additional files to generate.
    pub additional_files: Vec<FileFragment>,
}

/// A composable capability that can be added to any compatible template.
#[derive(Debug, Clone)]
pub struct CapabilityAddon {
    /// Addon name used in CLI (e.g., "telemetry", "auth").
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Cargo features required by this addon.
    pub required_features: Vec<&'static str>,
    /// Additional crate dependencies beyond `adk-rust`.
    pub additional_deps: Vec<DependencySpec>,
    /// Initialization priority (lower = earlier). Determines order in `main.rs`.
    pub init_priority: u8,
    /// Other addons that conflict with this one.
    pub incompatible_with: Vec<&'static str>,
    /// Code fragments for this addon.
    pub code_fragments: AddonCodeFragments,
}
