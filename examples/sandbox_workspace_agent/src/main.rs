use std::collections::HashMap;
use std::sync::Arc;

use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content};
use adk_model::gemini::GeminiModel;
use adk_runner::Runner;
use adk_runner::sandbox_runner::SandboxRunner;
use adk_sandbox::workspace::{ManifestEntry, SandboxSession};
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use clap::Parser;
use futures::StreamExt;
use tracing_subscriber::EnvFilter;

mod config;
mod display;

use config::{build_manifest, build_sandbox_config};
use display::{banner, print_event, print_summary};

/// Sandbox Workspace Agent Example
///
/// Demonstrates the full sandbox-agent-harness lifecycle:
/// Manifest → Provision → Session → Agent loop → Stop → Snapshot
#[derive(Parser, Debug)]
#[command(name = "sandbox-workspace-agent")]
#[command(about = "Demonstrates the sandbox-agent-harness lifecycle")]
pub struct CliArgs {
    /// Use DockerClient instead of LocalUnixClient for sandbox isolation.
    #[arg(long)]
    pub docker: bool,

    /// Enable snapshot/resume demonstration after the agent loop completes.
    #[arg(long)]
    pub snapshot: bool,
}

const AGENT_INSTRUCTIONS: &str = "\
You are a Rust developer assistant working inside a sandbox workspace. \
Your task is to create a simple Rust hello-world project. \
\
Steps: \
1. Use list_dir to see the current workspace contents \
2. Use write_file to create hello-world/Cargo.toml with a basic package definition \
3. Use write_file to create hello-world/src/main.rs with fn main() that prints \"Hello, world!\" \
4. Use exec_command to run 'cargo build' in the hello-world directory \
5. Use exec_command to run the compiled binary at hello-world/target/debug/hello-world \
\
Use only the provided tools. Do not skip steps.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignore errors if not found)
    let _ = dotenvy::dotenv();

    // Initialize tracing with RUST_LOG support, defaulting to "info"
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Parse CLI arguments
    let args = CliArgs::parse();

    // Validate GOOGLE_API_KEY environment variable
    let api_key = std::env::var("GOOGLE_API_KEY").map_err(|_| {
        anyhow::anyhow!(
            "GOOGLE_API_KEY environment variable is required.\n\
             Set it in .env or export it: export GOOGLE_API_KEY=your-key-here"
        )
    })?;

    // Print startup banner and configuration
    banner("Sandbox Workspace Agent Example");
    println!("  Backend:  {}", if args.docker { "DockerClient" } else { "LocalUnixClient" });
    println!("  Snapshot: {}", if args.snapshot { "enabled" } else { "disabled" });

    // ─── Phase 1: Manifest Definition ───────────────────────────────────────────
    banner("Phase 1: Manifest Definition");
    let manifest = build_manifest();
    println!("  Manifest entries:");
    for entry in &manifest.entries {
        match entry {
            ManifestEntry::Directory { path } => {
                println!("    📁 {path}/");
            }
            ManifestEntry::File { path, .. } => {
                println!("    📄 {path}");
            }
            ManifestEntry::GitRepo { url, path, .. } => {
                println!("    🔗 {path} (from {url})");
            }
            _ => {
                println!("    ❓ (unknown entry type)");
            }
        }
    }

    // ─── Phase 2: SandboxConfig Construction ────────────────────────────────────
    banner("Phase 2: SandboxConfig Construction");
    let sandbox_config = build_sandbox_config(&args).await?;
    println!("  Capabilities: Shell, Filesystem");
    println!("  Session timeout: {:?}", sandbox_config.session_timeout);
    println!("  Command timeout: {:?}", sandbox_config.command_timeout);
    println!("  Snapshot on stop: {}", sandbox_config.snapshot_on_stop);

    // ─── Phase 3: Provisioning Workspace ────────────────────────────────────────
    banner("Phase 3: Provisioning Workspace");
    println!("  Provisioning workspace from manifest...");

    let handle = sandbox_config
        .client
        .provision(&sandbox_config.manifest)
        .await
        .map_err(|e| anyhow::anyhow!("Provisioning failed: {e}"))?;

    println!("  ✅ SessionHandle: {}", handle.0);

    // Start the session
    println!("  Starting sandbox session...");
    let session: Box<dyn SandboxSession> = sandbox_config
        .client
        .start(&handle)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start session: {e}"))?;

    // Bind tools based on capabilities
    let session_arc: Arc<dyn SandboxSession> = Arc::from(session);
    let bound_tools = adk_runner::sandbox_runner::binding::bind_tools(
        Arc::clone(&session_arc),
        &sandbox_config.capabilities,
        sandbox_config.command_timeout,
    );

    println!("  ✅ Bound {} tool(s):", bound_tools.len());
    for tool in &bound_tools {
        println!("    🔧 {}", tool.name());
    }

    // ─── Phase 4: Agent Execution ───────────────────────────────────────────────
    banner("Phase 4: Agent Execution");
    println!("  Building LlmAgent with Gemini model...");

    let model = GeminiModel::new(&api_key, "gemini-2.5-flash")
        .map_err(|e| anyhow::anyhow!("Failed to create Gemini model: {e}"))?;

    let mut agent_builder = LlmAgentBuilder::new("sandbox-workspace-agent")
        .model(Arc::new(model))
        .instruction(AGENT_INSTRUCTIONS);

    for t in bound_tools {
        agent_builder = agent_builder.tool(t);
    }

    let agent = agent_builder.build().map_err(|e| anyhow::anyhow!("Failed to build agent: {e}"))?;

    println!("  ✅ Agent built: {}", agent.name());

    // Create session service and session
    let session_service: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    session_service
        .create(CreateRequest {
            app_name: "sandbox-workspace-agent".to_string(),
            user_id: "demo-user".to_string(),
            session_id: Some("session-1".to_string()),
            state: HashMap::new(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create session: {e}"))?;

    // Build the Runner
    let runner = Runner::builder()
        .app_name("sandbox-workspace-agent")
        .agent(Arc::new(agent))
        .session_service(session_service)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build runner: {e}"))?;

    // Create SandboxRunner wrapping the Runner (demonstrates the intended API)
    let sandbox_runner = SandboxRunner::new(runner);

    println!("  ✅ Runner and SandboxRunner constructed");
    println!("  Running agent loop...\n");

    // Run the agent via the inner runner with a user message
    let user_content =
        Content::new("user").with_text("Create the Rust hello-world project as instructed.");

    let run_result = sandbox_runner.inner().run_str("demo-user", "session-1", user_content).await;

    // Process events from the agent loop
    let agent_succeeded = match run_result {
        Ok(mut event_stream) => {
            let mut success = true;
            while let Some(event_result) = event_stream.next().await {
                match event_result {
                    Ok(event) => print_event(&event),
                    Err(e) => {
                        println!("  ❌ Event error: {e}");
                        success = false;
                    }
                }
            }
            success
        }
        Err(e) => {
            println!("  ❌ Agent execution failed: {e}");
            false
        }
    };

    // ─── Always stop the session (cleanup guarantee) ────────────────────────────
    println!("\n  Stopping sandbox session...");
    if let Err(e) = sandbox_config.client.stop(&handle).await {
        println!("  ⚠️  Stop failed (non-fatal): {e}");
    } else {
        println!("  ✅ Session stopped");
    }

    // ─── Phase 5: Results ───────────────────────────────────────────────────────
    banner("Phase 5: Results");
    if agent_succeeded {
        println!("  ✅ Agent execution completed successfully");
    } else {
        println!("  ❌ Agent execution failed");
    }

    // Snapshot if enabled
    let mut snapshot_id = None;
    if sandbox_config.snapshot_on_stop {
        println!("  📸 Creating snapshot...");
        match sandbox_config.client.snapshot(&handle).await {
            Ok(id) => {
                println!("  ✅ SnapshotId: {}", id.0);
                snapshot_id = Some(id);
            }
            Err(e) => {
                println!("  ⚠️  Snapshot failed: {e}");
            }
        }
    }

    // ─── Phase 6: Snapshot/Resume Verification (optional) ───────────────────────
    if args.snapshot {
        banner("Phase 6: Snapshot/Resume Verification");
        if let Some(ref snap_id) = snapshot_id {
            println!("  Resuming from snapshot: {}", snap_id.0);
            match sandbox_config.client.resume(snap_id).await {
                Ok(resumed_handle) => {
                    println!("  ✅ Resumed session: {}", resumed_handle.0);
                    match sandbox_config.client.start(&resumed_handle).await {
                        Ok(resumed_session) => {
                            // Verify workspace contents with list_dir
                            match resumed_session.list_dir("hello-world").await {
                                Ok(entries) => {
                                    println!("  Workspace contents after resume:");
                                    for entry in &entries {
                                        println!("    {:?} {}", entry.entry_type, entry.name);
                                    }
                                }
                                Err(e) => println!("  ⚠️  list_dir failed: {e}"),
                            }
                            // Stop the resumed session
                            let _ = sandbox_config.client.stop(&resumed_handle).await;
                        }
                        Err(e) => println!("  ❌ Failed to start resumed session: {e}"),
                    }
                }
                Err(e) => println!("  ❌ Resume failed: {e}"),
            }
        } else {
            println!("  ⚠️  No snapshot available for resume verification");
        }
    }

    // ─── Summary ────────────────────────────────────────────────────────────────
    let phases: Vec<(&str, bool)> = vec![
        ("Manifest definition", true),
        ("SandboxConfig construction", true),
        ("Provisioning", true),
        ("Agent execution", agent_succeeded),
        ("Stop/cleanup", true),
        ("Snapshot", !args.snapshot || snapshot_id.is_some()),
    ];
    print_summary(&phases);

    Ok(())
}
