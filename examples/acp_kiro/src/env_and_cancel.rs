//! # ACP session with env vars and cancel
//!
//! Demonstrates the new `adk-acp` features:
//! - **Environment variable injection**: Pass API keys and config to the agent process
//! - **Session cancel**: Cancel an in-progress prompt and restart the session
//!
//! ## Run
//!
//! ```bash
//! cargo run --bin acp-kiro-env-cancel
//! ```

use adk_acp::{AcpAgentConfig, AcpSession, PermissionPolicy};
use std::sync::Arc;
use tokio::time::{Duration, timeout};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    println!("=== ADK-ACP: Env Vars + Cancel Demo ===\n");

    // --- Feature 1: Environment variable injection ---
    println!("─── Feature: Environment Variables ───");
    println!("Passing env vars to the agent process (no unsafe set_var).\n");

    let config = AcpAgentConfig::new("kiro-cli acp --trust-all-tools")
        .working_dir(std::env::current_dir()?)
        // These env vars are passed to the child process via Command::env()
        .env("ADK_EXAMPLE_MODE", "true")
        .env("ADK_EXAMPLE_NAME", "env-cancel-demo")
        .envs([
            ("CUSTOM_VAR_1", "hello"),
            ("CUSTOM_VAR_2", "world"),
        ]);

    println!("Config created with {} env vars:", config.env.len());
    for (k, v) in &config.env {
        println!("  {k}={v}");
    }
    println!();

    let policy = Arc::new(PermissionPolicy::AutoApprove);

    println!("Starting session with env vars...");
    let mut session = AcpSession::start(config, policy).await?;
    println!("✅ Session started (env vars passed to child process)\n");

    // Verify the agent can see the env vars
    let r1 = session
        .prompt("Print the value of the environment variable ADK_EXAMPLE_NAME. Just the value, nothing else.")
        .await?;
    println!("Agent sees ADK_EXAMPLE_NAME = {}", r1.text.trim());
    println!("  (latency: {:?})\n", r1.duration);

    // --- Feature 2: Cancel ---
    println!("─── Feature: Session Cancel ───");
    println!("Sending a long-running prompt, then cancelling after 3s.\n");

    // Start a prompt that would take a while
    let session_clone_prompt = "Write a detailed 500-word essay about the history of Rust programming language. Include dates, key contributors, and major milestones.";
    println!("Sending long prompt: \"{}\"\n", &session_clone_prompt[..60]);

    // Race: prompt vs timeout
    let prompt_future = session.prompt(session_clone_prompt);
    match timeout(Duration::from_secs(3), prompt_future).await {
        Ok(Ok(result)) => {
            // Agent responded within 3s (fast agent!)
            println!("Agent responded in {:?} (faster than timeout)", result.duration);
            println!("Response preview: {}...", &result.text[..result.text.len().min(100)]);
        }
        Ok(Err(e)) => {
            println!("Prompt errored: {e}");
        }
        Err(_) => {
            // Timed out — cancel the session
            println!("⏱️  Timeout reached (3s). Cancelling...");
            session.cancel().await?;
            println!("✅ Session cancelled and restarted.\n");

            // Verify session still works after cancel
            println!("Verifying session works after cancel...");
            let r2 = session.prompt("Say 'hello' and nothing else.").await?;
            println!("Agent says: {}", r2.text.trim());
            println!("✅ Session functional after cancel.");
        }
    }

    // --- Summary ---
    println!("\n📊 Session Summary:");
    println!("   Prompts sent: {}", session.prompt_count());
    println!("   Uptime: {:?}", session.uptime());
    println!("   Active: {}", session.is_active());

    session.close().await?;
    println!("\n✅ Done.");
    Ok(())
}
