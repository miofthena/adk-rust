//! Repo-internal task runner — works in any shell (bash, zsh, PowerShell, cmd)
//! because the only prerequisite is cargo itself.
//!
//! ```text
//! cargo xtask publish              # native: cargo publish --workspace
//! cargo xtask publish --dry-run    # native, no upload
//! cargo xtask publish --resume     # sequential per-crate publish in computed
//!                                  # dependency order, skipping versions that
//!                                  # are already on crates.io
//! cargo xtask publish --resume --dry-run   # print the computed order only
//! ```
//!
//! The publish order is computed from `cargo metadata` at runtime (normal +
//! build deps, plus dev-deps that carry a version requirement), so there is no
//! hand-maintained tier list to go stale. `publish = false` crates (like this
//! one) are excluded automatically.

use std::collections::{BTreeMap, BTreeSet};
use std::process::{exit, Command};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("publish") => publish(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask publish [--resume] [--dry-run]");
            exit(2);
        }
    }
}

fn publish(flags: &[String]) {
    let mut resume = false;
    let mut dry_run = false;
    for flag in flags {
        match flag.as_str() {
            "--resume" => resume = true,
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("unknown flag: {other}");
                exit(2);
            }
        }
    }

    if !resume {
        println!("=== Publishing ADK-Rust (cargo publish --workspace) ===");
        println!("If this fails partway, finish with: cargo xtask publish --resume\n");
        let mut cmd = Command::new("cargo");
        cmd.arg("publish").arg("--workspace");
        if dry_run {
            cmd.arg("--dry-run");
        }
        let status = cmd.status().expect("failed to run cargo");
        exit(status.code().unwrap_or(1));
    }

    // ── Resume mode: sequential per-crate publish in dependency order ──────
    let order = publish_order();
    println!("=== Publishing ADK-Rust (sequential resume) ===");
    println!("Crates ({} total, dependency order):", order.len());
    for (i, c) in order.iter().enumerate() {
        println!("  {:>2}. {c}", i + 1);
    }
    if dry_run {
        println!("\n--dry-run: no crates were published.");
        return;
    }

    let mut published = 0u32;
    let mut skipped = 0u32;
    let mut failed: Vec<String> = Vec::new();

    for (i, krate) in order.iter().enumerate() {
        println!("\n📦 [{}/{}] Publishing: {krate}", i + 1, order.len());
        let output = Command::new("cargo")
            .args(["publish", "-p", krate])
            .output()
            .expect("failed to run cargo");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{combined}");

        if combined.contains("already exists") || combined.contains("already uploaded") {
            println!("⏭  Already published");
            skipped += 1;
        } else if output.status.success() {
            println!("✅ Published — waiting for crates.io indexing...");
            published += 1;
            std::thread::sleep(std::time::Duration::from_secs(15));
        } else {
            println!("❌ FAILED (exit {:?})", output.status.code());
            failed.push(krate.clone());
        }
    }

    println!("\n=== SUMMARY ===");
    println!("✅ Published: {published}");
    println!("⏭  Skipped:   {skipped}");
    println!("❌ Failed:    {}", failed.len());
    for c in &failed {
        println!("  - {c}");
    }
    if !failed.is_empty() {
        exit(1);
    }
}

/// Compute a publish order for all publishable workspace crates from
/// `cargo metadata`: every crate comes after its workspace-internal normal and
/// build dependencies, and after dev-dependencies that carry a version
/// requirement (path-only dev-deps are stripped at publish and ignored).
fn publish_order() -> Vec<String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("failed to run cargo metadata");
    if !output.status.success() {
        eprintln!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));
        exit(1);
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid cargo metadata JSON");

    let packages = meta["packages"].as_array().expect("packages array");
    let mut publishable = BTreeSet::new();
    for p in packages {
        // `publish` is null when unrestricted; `[]`/false-equivalent means never publish.
        let restricted = p["publish"].as_array().is_some_and(|registries| registries.is_empty());
        if !restricted {
            publishable.insert(p["name"].as_str().expect("name").to_string());
        }
    }

    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in packages {
        let name = p["name"].as_str().unwrap().to_string();
        if !publishable.contains(&name) {
            continue;
        }
        let entry = deps.entry(name).or_default();
        for d in p["dependencies"].as_array().unwrap() {
            let dep_name = d["name"].as_str().unwrap();
            if !publishable.contains(dep_name) {
                continue;
            }
            let kind = d["kind"].as_str().unwrap_or("normal");
            let versioned = d["req"].as_str().unwrap_or("*") != "*";
            if kind == "normal" || kind == "build" || (kind == "dev" && versioned) {
                entry.insert(dep_name.to_string());
            }
        }
    }

    // Kahn's algorithm (deterministic: BTree keeps name order within a tier).
    let mut order = Vec::with_capacity(deps.len());
    let mut remaining = deps;
    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|(_, ds)| ds.iter().all(|d| !remaining.contains_key(d)))
            .map(|(name, _)| name.clone())
            .collect();
        if ready.is_empty() {
            eprintln!("dependency cycle among: {:?}", remaining.keys().collect::<Vec<_>>());
            exit(1);
        }
        for name in ready {
            remaining.remove(&name);
            order.push(name);
        }
    }
    order
}
