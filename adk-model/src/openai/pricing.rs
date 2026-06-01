//! Token pricing for OpenAI models (March 2026).
//!
//! Provides per-model cost calculation based on token counts.
//!
//! OpenAI models have automatic prompt caching with varying discount tiers:
//! - GPT-5 family: 90% off cached input reads
//! - GPT-4.1 family: 75% off cached input reads
//! - GPT-4o / o-series: 50% off cached input reads
//!
//! # Example
//!
//! ```rust
//! use adk_model::openai::pricing::{OpenAIPricing, estimate_cost};
//!
//! let cost = estimate_cost(&OpenAIPricing::GPT_41, 50_000, 1_000, 10_000);
//! println!("Total: ${:.6}", cost.total());
//! ```

/// Per-million-token prices for a single OpenAI model.
///
/// All values are in USD per 1 million tokens.
#[derive(Debug, Clone, Copy)]
pub struct OpenAIPricing {
    /// Input token price ($/MTok).
    pub input: f64,
    /// Cached input token price ($/MTok).
    pub cached_input: f64,
    /// Output token price ($/MTok).
    pub output: f64,
}

impl OpenAIPricing {
    // ── GPT-5.5 family (90% cache discount) ──

    /// GPT-5.5 — next-generation flagship model.
    pub const GPT_55: Self = Self { input: 3.00, cached_input: 0.30, output: 18.00 };

    /// GPT-5.5 Pro — highest capability tier.
    pub const GPT_55_PRO: Self = Self { input: 6.00, cached_input: 0.60, output: 36.00 };

    /// GPT-5.5 Instant — fast, cost-effective GPT-5.5-class model.
    pub const GPT_55_INSTANT: Self = Self { input: 0.50, cached_input: 0.05, output: 3.00 };

    // ── GPT-5.4 family (90% cache discount) ──

    /// GPT-5.4 — most capable model for professional work.
    pub const GPT_54: Self = Self { input: 2.00, cached_input: 0.20, output: 14.00 };

    /// GPT-5.4 Mini — strongest mini model for coding, computer use, subagents.
    pub const GPT_54_MINI: Self = Self { input: 0.80, cached_input: 0.08, output: 6.00 };

    /// GPT-5.4 Nano — cheapest GPT-5.4-class model for high-volume tasks.
    pub const GPT_54_NANO: Self = Self { input: 0.20, cached_input: 0.02, output: 1.50 };

    /// GPT-5.4 Pro — premium GPT-5.4-class model.
    pub const GPT_54_PRO: Self = Self { input: 4.00, cached_input: 0.40, output: 28.00 };

    // ── GPT-5.3 family (90% cache discount) ──

    /// GPT-5.3 Codex — code-optimized model.
    pub const GPT_53_CODEX: Self = Self { input: 1.50, cached_input: 0.15, output: 12.00 };

    /// GPT-5.3 Chat Latest — latest chat-optimized model.
    pub const GPT_53_CHAT_LATEST: Self = Self { input: 1.50, cached_input: 0.15, output: 12.00 };

    // ── GPT-5.2 family (90% cache discount) ──

    /// GPT-5.2 — general-purpose model.
    pub const GPT_52: Self = Self { input: 1.25, cached_input: 0.125, output: 10.00 };

    /// GPT-5.2 Codex — code-optimized GPT-5.2 variant.
    pub const GPT_52_CODEX: Self = Self { input: 1.25, cached_input: 0.125, output: 10.00 };

    // ── GPT-5.1 family (90% cache discount) ──

    /// GPT-5.1 — general-purpose model.
    pub const GPT_51: Self = Self { input: 1.00, cached_input: 0.10, output: 8.00 };

    /// GPT-5.1 Codex — code-optimized GPT-5.1 variant.
    pub const GPT_51_CODEX: Self = Self { input: 1.00, cached_input: 0.10, output: 8.00 };

    /// GPT-5.1 Codex Max — high-throughput code model.
    pub const GPT_51_CODEX_MAX: Self = Self { input: 2.00, cached_input: 0.20, output: 16.00 };

    /// GPT-5.1 Codex Mini — budget code model.
    pub const GPT_51_CODEX_MINI: Self = Self { input: 0.30, cached_input: 0.03, output: 2.40 };

    // ── GPT-5 family (90% cache discount) ──

    /// GPT-5 — flagship agentic model.
    pub const GPT_5: Self = Self { input: 2.50, cached_input: 0.25, output: 15.00 };

    /// GPT-5 Mini — budget GPT-5-class model.
    pub const GPT_5_MINI: Self = Self { input: 0.60, cached_input: 0.06, output: 4.00 };

    /// GPT-5 Nano — cheapest GPT-5-class model.
    pub const GPT_5_NANO: Self = Self { input: 0.15, cached_input: 0.015, output: 1.00 };

    /// GPT-5 Pro — premium GPT-5-class model.
    pub const GPT_5_PRO: Self = Self { input: 5.00, cached_input: 0.50, output: 30.00 };

    // ── GPT-4.1 family (75% cache discount) ──

    /// GPT-4.1 — production workhorse, 1M context window.
    pub const GPT_41: Self = Self { input: 2.00, cached_input: 0.50, output: 8.00 };

    /// GPT-4.1 Mini — mid-tier production tasks, 1M context.
    pub const GPT_41_MINI: Self = Self { input: 0.40, cached_input: 0.10, output: 1.60 };

    /// GPT-4.1 Nano — classification, routing, extraction, 1M context.
    pub const GPT_41_NANO: Self = Self { input: 0.10, cached_input: 0.025, output: 0.40 };

    // ── o-series reasoning models (50% cache discount) ──

    /// o3 — advanced reasoning model.
    pub const O3: Self = Self { input: 2.00, cached_input: 0.50, output: 8.00 };

    /// o4-mini — best-value reasoning model.
    pub const O4_MINI: Self = Self { input: 1.10, cached_input: 0.275, output: 4.40 };

    /// o3-mini — legacy reasoning model.
    pub const O3_MINI: Self = Self { input: 1.10, cached_input: 0.55, output: 4.40 };

    /// o1 — legacy deep reasoning model.
    pub const O1: Self = Self { input: 15.00, cached_input: 7.50, output: 60.00 };

    // ── GPT-4o family (50% cache discount, legacy) ──

    /// GPT-4o — legacy production model.
    pub const GPT_4O: Self = Self { input: 2.50, cached_input: 1.25, output: 10.00 };

    /// GPT-4o Mini — legacy simple tasks.
    pub const GPT_4O_MINI: Self = Self { input: 0.15, cached_input: 0.075, output: 0.60 };

    // ── Realtime models ──

    /// GPT-Realtime-1.5 — text pricing (audio is separate).
    ///
    /// Audio: input $32/MTok, cached $0.40/MTok, output $64/MTok.
    /// Image: input $5/MTok, cached $0.50/MTok.
    pub const GPT_REALTIME_15_TEXT: Self = Self { input: 4.00, cached_input: 0.40, output: 16.00 };

    /// GPT-Realtime-1.5 — audio pricing.
    pub const GPT_REALTIME_15_AUDIO: Self =
        Self { input: 32.00, cached_input: 0.40, output: 64.00 };

    // ── Image generation ──

    /// GPT-Image-1.5 — text pricing.
    pub const GPT_IMAGE_15_TEXT: Self = Self { input: 5.00, cached_input: 1.25, output: 10.00 };

    /// GPT-Image-1.5 — image pricing.
    pub const GPT_IMAGE_15_IMAGE: Self = Self { input: 8.00, cached_input: 2.00, output: 32.00 };

    // ── GPT Image 2 ──

    /// GPT-Image-2 — text pricing.
    pub const GPT_IMAGE_2_TEXT: Self = Self { input: 5.00, cached_input: 1.25, output: 10.00 };

    /// GPT-Image-2 — image pricing.
    pub const GPT_IMAGE_2_IMAGE: Self = Self { input: 8.00, cached_input: 2.00, output: 32.00 };

    // ── Deep research models ──

    /// o3 Deep Research — extended reasoning for research tasks.
    pub const O3_DEEP_RESEARCH: Self = Self { input: 2.00, cached_input: 0.50, output: 8.00 };

    /// o4-mini Deep Research — cost-effective deep research model.
    pub const O4_MINI_DEEP_RESEARCH: Self = Self { input: 1.10, cached_input: 0.275, output: 4.40 };
}

/// Itemised cost breakdown from a single API call.
#[derive(Debug, Clone, Copy, Default)]
pub struct CostBreakdown {
    /// Cost of uncached input tokens.
    pub input_cost: f64,
    /// Cost of cached input tokens.
    pub cache_cost: f64,
    /// Cost of output tokens.
    pub output_cost: f64,
}

impl CostBreakdown {
    /// Total cost in USD.
    pub fn total(&self) -> f64 {
        self.input_cost + self.cache_cost + self.output_cost
    }
}

impl std::fmt::Display for CostBreakdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "${:.6} (in=${:.6} cache=${:.6} out=${:.6})",
            self.total(),
            self.input_cost,
            self.cache_cost,
            self.output_cost
        )
    }
}

/// Estimate the cost of a single API call.
///
/// # Arguments
///
/// * `pricing` - The model's pricing tier
/// * `input_tokens` - Number of uncached input tokens
/// * `output_tokens` - Number of output tokens (includes reasoning tokens for o-series)
/// * `cached_tokens` - Number of tokens served from cache
///
/// # Example
///
/// ```rust
/// use adk_model::openai::pricing::{OpenAIPricing, estimate_cost};
///
/// let cost = estimate_cost(&OpenAIPricing::GPT_41, 50_000, 1_000, 10_000);
/// println!("Total: ${:.6}", cost.total());
/// ```
pub fn estimate_cost(
    pricing: &OpenAIPricing,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
) -> CostBreakdown {
    let mtok = 1_000_000.0;
    CostBreakdown {
        input_cost: input_tokens as f64 / mtok * pricing.input,
        cache_cost: cached_tokens as f64 / mtok * pricing.cached_input,
        output_cost: output_tokens as f64 / mtok * pricing.output,
    }
}

/// Estimate batch API cost (50% off all token costs).
pub fn estimate_batch_cost(
    pricing: &OpenAIPricing,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
) -> CostBreakdown {
    let mtok = 1_000_000.0;
    CostBreakdown {
        input_cost: input_tokens as f64 / mtok * pricing.input * 0.5,
        cache_cost: cached_tokens as f64 / mtok * pricing.cached_input * 0.5,
        output_cost: output_tokens as f64 / mtok * pricing.output * 0.5,
    }
}

/// Look up pricing for a model by its identifier string.
///
/// Returns `None` for unknown models, allowing callers to use a zero-cost fallback.
///
/// # Arguments
///
/// * `model_name` - The model identifier (e.g., "gpt-5.5", "o3-deep-research")
///
/// # Example
///
/// ```rust
/// use adk_model::openai::pricing::lookup_pricing;
///
/// let pricing = lookup_pricing("gpt-5.5");
/// assert!(pricing.is_some());
///
/// let unknown = lookup_pricing("unknown-model");
/// assert!(unknown.is_none());
/// ```
pub fn lookup_pricing(model_name: &str) -> Option<&'static OpenAIPricing> {
    match model_name {
        // GPT-5.5 family
        "gpt-5.5" => Some(&OpenAIPricing::GPT_55),
        "gpt-5.5-pro" => Some(&OpenAIPricing::GPT_55_PRO),
        "gpt-5.5-instant" => Some(&OpenAIPricing::GPT_55_INSTANT),

        // GPT-5.4 family
        "gpt-5.4" => Some(&OpenAIPricing::GPT_54),
        "gpt-5.4-mini" => Some(&OpenAIPricing::GPT_54_MINI),
        "gpt-5.4-nano" => Some(&OpenAIPricing::GPT_54_NANO),
        "gpt-5.4-pro" => Some(&OpenAIPricing::GPT_54_PRO),

        // GPT-5.3 family
        "gpt-5.3-codex" => Some(&OpenAIPricing::GPT_53_CODEX),
        "gpt-5.3-chat-latest" => Some(&OpenAIPricing::GPT_53_CHAT_LATEST),

        // GPT-5.2 family
        "gpt-5.2" => Some(&OpenAIPricing::GPT_52),
        "gpt-5.2-codex" => Some(&OpenAIPricing::GPT_52_CODEX),

        // GPT-5.1 family
        "gpt-5.1" => Some(&OpenAIPricing::GPT_51),
        "gpt-5.1-codex" => Some(&OpenAIPricing::GPT_51_CODEX),
        "gpt-5.1-codex-max" => Some(&OpenAIPricing::GPT_51_CODEX_MAX),
        "gpt-5.1-codex-mini" => Some(&OpenAIPricing::GPT_51_CODEX_MINI),

        // GPT-5 family
        "gpt-5" => Some(&OpenAIPricing::GPT_5),
        "gpt-5-mini" => Some(&OpenAIPricing::GPT_5_MINI),
        "gpt-5-nano" => Some(&OpenAIPricing::GPT_5_NANO),
        "gpt-5-pro" => Some(&OpenAIPricing::GPT_5_PRO),

        // GPT-4.1 family
        "gpt-4.1" => Some(&OpenAIPricing::GPT_41),
        "gpt-4.1-mini" => Some(&OpenAIPricing::GPT_41_MINI),
        "gpt-4.1-nano" => Some(&OpenAIPricing::GPT_41_NANO),

        // o-series reasoning models
        "o3" => Some(&OpenAIPricing::O3),
        "o4-mini" => Some(&OpenAIPricing::O4_MINI),
        "o3-mini" => Some(&OpenAIPricing::O3_MINI),
        "o1" => Some(&OpenAIPricing::O1),

        // Deep research models
        "o3-deep-research" => Some(&OpenAIPricing::O3_DEEP_RESEARCH),
        "o4-mini-deep-research" => Some(&OpenAIPricing::O4_MINI_DEEP_RESEARCH),

        // GPT-4o family (legacy)
        "gpt-4o" => Some(&OpenAIPricing::GPT_4O),
        "gpt-4o-mini" => Some(&OpenAIPricing::GPT_4O_MINI),

        // Realtime models
        "gpt-realtime-1.5" => Some(&OpenAIPricing::GPT_REALTIME_15_TEXT),
        "gpt-realtime-1.5-audio" => Some(&OpenAIPricing::GPT_REALTIME_15_AUDIO),

        // Image generation models
        "gpt-image-1.5" => Some(&OpenAIPricing::GPT_IMAGE_15_TEXT),
        "gpt-image-1.5-image" => Some(&OpenAIPricing::GPT_IMAGE_15_IMAGE),
        "gpt-image-2" => Some(&OpenAIPricing::GPT_IMAGE_2_TEXT),
        "gpt-image-2-image" => Some(&OpenAIPricing::GPT_IMAGE_2_IMAGE),

        // Unknown model — return None for zero-cost fallback
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_41_basic_cost() {
        let cost = estimate_cost(&OpenAIPricing::GPT_41, 1_000_000, 1_000_000, 0);
        assert!((cost.input_cost - 2.00).abs() < 1e-9);
        assert!((cost.output_cost - 8.00).abs() < 1e-9);
        assert!((cost.total() - 10.00).abs() < 1e-9);
    }

    #[test]
    fn gpt_41_with_cache() {
        let cost = estimate_cost(&OpenAIPricing::GPT_41, 500_000, 100_000, 500_000);
        // 500K input @ $2.00/MTok = $1.00
        assert!((cost.input_cost - 1.00).abs() < 1e-9);
        // 500K cached @ $0.50/MTok = $0.25
        assert!((cost.cache_cost - 0.25).abs() < 1e-9);
        // 100K output @ $8.00/MTok = $0.80
        assert!((cost.output_cost - 0.80).abs() < 1e-9);
        assert!((cost.total() - 2.05).abs() < 1e-9);
    }

    #[test]
    fn gpt_5_cache_discount_90_percent() {
        // GPT-5: input $2.50, cached $0.25 (90% off)
        let cost = estimate_cost(&OpenAIPricing::GPT_5, 0, 0, 1_000_000);
        assert!((cost.cache_cost - 0.25).abs() < 1e-9);
    }

    #[test]
    fn o4_mini_reasoning_cost() {
        // o4-mini: 1M input + 5M output (reasoning tokens count as output)
        let cost = estimate_cost(&OpenAIPricing::O4_MINI, 1_000_000, 5_000_000, 0);
        assert!((cost.input_cost - 1.10).abs() < 1e-9);
        assert!((cost.output_cost - 22.00).abs() < 1e-9);
    }

    #[test]
    fn batch_50_percent_discount() {
        let standard = estimate_cost(&OpenAIPricing::GPT_41, 1_000_000, 1_000_000, 0);
        let batch = estimate_batch_cost(&OpenAIPricing::GPT_41, 1_000_000, 1_000_000, 0);
        assert!((batch.total() - standard.total() * 0.5).abs() < 1e-9);
    }

    #[test]
    fn gpt_41_nano_cheapest() {
        let cost = estimate_cost(&OpenAIPricing::GPT_41_NANO, 1_000_000, 1_000_000, 0);
        assert!((cost.input_cost - 0.10).abs() < 1e-9);
        assert!((cost.output_cost - 0.40).abs() < 1e-9);
    }

    #[test]
    fn zero_tokens_zero_cost() {
        let cost = estimate_cost(&OpenAIPricing::GPT_5, 0, 0, 0);
        assert!((cost.total() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn display_format() {
        let cost = CostBreakdown { input_cost: 0.003, cache_cost: 0.001, output_cost: 0.0075 };
        let s = cost.to_string();
        assert!(s.starts_with('$'));
        assert!(s.contains("in="));
        assert!(s.contains("cache="));
        assert!(s.contains("out="));
    }

    #[test]
    fn lookup_known_models() {
        assert!(lookup_pricing("gpt-5.5").is_some());
        assert!(lookup_pricing("gpt-5.5-pro").is_some());
        assert!(lookup_pricing("gpt-5.5-instant").is_some());
        assert!(lookup_pricing("gpt-5.4").is_some());
        assert!(lookup_pricing("gpt-5.4-mini").is_some());
        assert!(lookup_pricing("gpt-5.4-nano").is_some());
        assert!(lookup_pricing("gpt-5.4-pro").is_some());
        assert!(lookup_pricing("gpt-5.3-codex").is_some());
        assert!(lookup_pricing("gpt-5.3-chat-latest").is_some());
        assert!(lookup_pricing("gpt-5.2").is_some());
        assert!(lookup_pricing("gpt-5.2-codex").is_some());
        assert!(lookup_pricing("gpt-5.1").is_some());
        assert!(lookup_pricing("gpt-5.1-codex").is_some());
        assert!(lookup_pricing("gpt-5.1-codex-max").is_some());
        assert!(lookup_pricing("gpt-5.1-codex-mini").is_some());
        assert!(lookup_pricing("gpt-5").is_some());
        assert!(lookup_pricing("gpt-5-mini").is_some());
        assert!(lookup_pricing("gpt-5-nano").is_some());
        assert!(lookup_pricing("gpt-5-pro").is_some());
        assert!(lookup_pricing("gpt-image-2").is_some());
        assert!(lookup_pricing("gpt-image-2-image").is_some());
        assert!(lookup_pricing("o3-deep-research").is_some());
        assert!(lookup_pricing("o4-mini-deep-research").is_some());
    }

    #[test]
    fn lookup_unknown_model_returns_none() {
        assert!(lookup_pricing("unknown-model").is_none());
        assert!(lookup_pricing("gpt-99").is_none());
        assert!(lookup_pricing("").is_none());
    }

    #[test]
    fn lookup_pricing_values_correct() {
        let p = lookup_pricing("gpt-5.5").unwrap();
        assert!((p.input - 3.00).abs() < 1e-9);
        assert!((p.cached_input - 0.30).abs() < 1e-9);
        assert!((p.output - 18.00).abs() < 1e-9);

        let p = lookup_pricing("o3-deep-research").unwrap();
        assert!((p.input - 2.00).abs() < 1e-9);
        assert!((p.cached_input - 0.50).abs() < 1e-9);
        assert!((p.output - 8.00).abs() < 1e-9);
    }
}
