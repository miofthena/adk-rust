//! Provider configuration for the composable scaffolding engine.
//!
//! Each LLM provider has a configuration that determines the feature flag,
//! environment variable, model initialization code, and default model.

/// Provider-specific configuration for code generation.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider name (e.g., "gemini", "openai").
    pub name: &'static str,
    /// Cargo feature flag to enable this provider.
    pub feature_flag: &'static str,
    /// Environment variable for the API key or endpoint.
    pub env_var: &'static str,
    /// Code snippet for model initialization in `main.rs`.
    pub model_init_code: &'static str,
    /// Default model identifier.
    pub default_model: &'static str,
    /// Whether this provider requires an API key.
    pub requires_api_key: bool,
}

/// All supported provider configurations.
static PROVIDERS: &[ProviderConfig] = &[
    ProviderConfig {
        name: "gemini",
        feature_flag: "gemini",
        env_var: "GOOGLE_API_KEY",
        model_init_code: "Gemini::from_env(\"gemini-2.5-flash\")",
        default_model: "gemini-2.5-flash",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "openai",
        feature_flag: "openai",
        env_var: "OPENAI_API_KEY",
        model_init_code: "OpenAI::from_env(\"gpt-5-mini\")",
        default_model: "gpt-5-mini",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "anthropic",
        feature_flag: "anthropic",
        env_var: "ANTHROPIC_API_KEY",
        model_init_code: "Anthropic::from_env(\"claude-sonnet-4-5-20250929\")",
        default_model: "claude-sonnet-4-5-20250929",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "deepseek",
        feature_flag: "deepseek",
        env_var: "DEEPSEEK_API_KEY",
        model_init_code: "DeepSeek::from_env(\"deepseek-chat\")",
        default_model: "deepseek-chat",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "ollama",
        feature_flag: "ollama",
        env_var: "",
        model_init_code: "Ollama::new(\"llama3.2\")",
        default_model: "llama3.2",
        requires_api_key: false,
    },
    ProviderConfig {
        name: "groq",
        feature_flag: "groq",
        env_var: "GROQ_API_KEY",
        model_init_code: "Groq::from_env(\"llama-3.3-70b-versatile\")",
        default_model: "llama-3.3-70b-versatile",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "openrouter",
        feature_flag: "openrouter",
        env_var: "OPENROUTER_API_KEY",
        model_init_code: "OpenRouter::from_env(\"openai/gpt-4o\")",
        default_model: "openai/gpt-4o",
        requires_api_key: true,
    },
    ProviderConfig {
        name: "bedrock",
        feature_flag: "bedrock",
        env_var: "AWS_REGION",
        model_init_code: "Bedrock::from_env(\"anthropic.claude-3-5-sonnet-20241022-v2:0\")",
        default_model: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        requires_api_key: false,
    },
    ProviderConfig {
        name: "azure-ai",
        feature_flag: "azure-ai",
        env_var: "AZURE_AI_ENDPOINT",
        model_init_code: "AzureAI::from_env(\"gpt-4o\")",
        default_model: "gpt-4o",
        requires_api_key: true,
    },
];

/// Look up a provider configuration by name.
///
/// # Errors
///
/// Returns an error string if the provider name is not recognized.
pub fn get_provider_config(provider: &str) -> Result<&'static ProviderConfig, String> {
    PROVIDERS.iter().find(|p| p.name == provider).ok_or_else(|| {
        let supported: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        format!("unknown provider '{provider}'. Supported: {}", supported.join(", "))
    })
}

/// Returns all registered provider configurations.
pub fn all_providers() -> &'static [ProviderConfig] {
    PROVIDERS
}
