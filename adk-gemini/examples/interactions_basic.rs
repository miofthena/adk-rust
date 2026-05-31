//! Basic usage of the Gemini Interactions API (Beta).
//!
//! Demonstrates three things:
//! 1. A single-turn interaction with `output_text()`.
//! 2. A streamed interaction accumulating `step.delta` text fragments.
//! 3. Server-side multi-turn continuation via `previous_interaction_id`.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY=... cargo run -p adk-gemini --features interactions --example interactions_basic
//! ```

use adk_gemini::{Gemini, Model, ThinkingLevel};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .expect("set GEMINI_API_KEY or GOOGLE_API_KEY");

    let gemini = Gemini::new(api_key)?;

    // ── 1. Single-turn interaction ──────────────────────────────────────
    println!("== single turn ==");
    let interaction = gemini
        .create_interaction()
        .model(Model::Gemini35Flash)
        .system_instruction("You are concise.")
        .input_text("What is the capital of France?")
        .thinking_level(ThinkingLevel::Low)
        .send()
        .await?;

    println!("status: {:?}", interaction.status);
    println!("output: {}", interaction.output_text().unwrap_or_default());
    if let Some(usage) = &interaction.usage {
        println!("tokens: {} total", usage.total_tokens);
    }

    // ── 2. Streamed interaction ─────────────────────────────────────────
    println!("\n== streaming ==");
    let mut stream = gemini
        .create_interaction()
        .model(Model::Gemini35Flash)
        .input_text("Write a haiku about Rust.")
        .stream()
        .await?;

    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let Some(fragment) = event?.text_delta() {
            print!("{fragment}");
            text.push_str(fragment);
        }
    }
    println!("\n(accumulated {} chars)", text.len());

    // ── 3. Server-side multi-turn continuation ──────────────────────────
    println!("\n== multi-turn (server-side history) ==");
    let first = gemini
        .create_interaction()
        .model(Model::Gemini35Flash)
        .input_text("My favorite color is teal. Remember it.")
        .send()
        .await?;
    println!("turn 1: {}", first.output_text().unwrap_or_default());

    // Note: tools/system_instruction/generation_config are interaction-scoped
    // and must be re-specified each turn; only history carries via the id.
    let second = gemini
        .create_interaction()
        .model(Model::Gemini35Flash)
        .previous_interaction_id(&first.id)
        .input_text("What is my favorite color?")
        .send()
        .await?;
    println!("turn 2: {}", second.output_text().unwrap_or_default());

    Ok(())
}
