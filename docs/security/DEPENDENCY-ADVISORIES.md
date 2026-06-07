# Dependency Security Advisories

## Overview

This document tracks known security advisories affecting ADK-Rust transitive dependencies. Each entry includes the advisory identifier, affected crate, severity assessment, impact on ADK-Rust, current status, and disposition (accepted risk, mitigation planned, or resolved).

The purpose of this file is to provide transparency to consumers about the security posture of ADK-Rust's dependency tree and to document informed risk-acceptance decisions where upstream fixes are unavailable or upgrades are deferred.

These accepted advisories are also configured in [`.cargo/audit.toml`](../../.cargo/audit.toml) so that `cargo audit` passes in CI while still surfacing new, unreviewed advisories.

**Last reviewed:** 2026-06-07

---

## Active Advisories

### RUSTSEC-2023-0071 — rsa: Marvin Attack (Timing Side-Channel)

- **Crate:** `rsa`
- **Severity:** Moderate
- **Advisory:** [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071)
- **ADK Impact:** Transitive dependency via `adk-auth` → `azure_security_keyvault` → `rsa`
- **Status:** No upstream fix available. The `rsa` crate maintainers are aware but have not released a patched version.
- **Disposition:** Accepted risk
- **Conditions:** The Marvin attack requires an attacker to perform precise timing measurements of RSA decryption operations. This is only exploitable when:
  1. The application performs RSA PKCS#1 v1.5 decryption (not signing)
  2. The attacker can submit arbitrary ciphertexts and observe decryption timing with high precision
  3. The attacker has local or adjacent network access with minimal latency jitter
- **ADK-Specific Context:** ADK-Rust uses the `rsa` crate transitively through Azure Key Vault operations. The typical usage pattern (key retrieval and signature verification) does not expose the vulnerable decryption path to attacker-controlled inputs.
- **Mitigation:** Monitor upstream for a fix. If consumers use Azure Key Vault RSA decryption with untrusted input, consider using a separate HSM-backed key.

---

### RUSTSEC-2026-0104, RUSTSEC-2026-0098, RUSTSEC-2026-0099 — rustls-webpki

- **Crate:** `rustls-webpki` (< 0.103.12)
- **Severity:** Moderate
- **Advisories:**
  - [RUSTSEC-2026-0104](https://rustsec.org/advisories/RUSTSEC-2026-0104) — CRL validation bypass
  - [RUSTSEC-2026-0098](https://rustsec.org/advisories/RUSTSEC-2026-0098) — Name constraint bypass
  - [RUSTSEC-2026-0099](https://rustsec.org/advisories/RUSTSEC-2026-0099) — Related name constraint issue
- **ADK Impact:** Transitive dependency via `adk-server`/`adk-auth` → `rustls` → `rustls-webpki`
- **Status:** Fix available in `rustls-webpki` ≥ 0.103.12, but upgrade is deferred — causes breaking compilation issues across the workspace due to incompatible `rustls` version constraints from multiple downstream crates.
- **Disposition:** Accepted risk — upgrade deferred to post-1.0 patch release
- **Conditions:** These vulnerabilities require specific TLS server configurations to be exploitable:
  1. CRL validation bypass (RUSTSEC-2026-0104): Only affects deployments that rely on Certificate Revocation Lists for client certificate validation
  2. Name constraint bypass (RUSTSEC-2026-0098, RUSTSEC-2026-0099): Only affects deployments using X.509 name constraints to restrict certificate issuance scope
- **ADK-Specific Context:** ADK-Rust's TLS usage is primarily outbound HTTPS connections to LLM provider APIs. Inbound TLS (via `adk-server`) typically terminates at a reverse proxy or load balancer, not at the application layer. The CRL and name-constraint features are rarely configured in typical ADK deployments.
- **Mitigation:** Upgrade to `rustls-webpki` ≥ 0.103.12 in the post-1.0.1 patch release once upstream `rustls` ecosystem version alignment is resolved. Consumers relying on CRL validation or name constraints in their TLS configuration should use an external TLS termination proxy.

---

### lru 0.12.5 — Unsoundness

- **Crate:** `lru` (0.12.5)
- **Severity:** Low (memory safety, but requires specific usage patterns)
- **ADK Impact:** Transitive dependency via `adk-rag` → `tantivy`/`lancedb` → `lru`
- **Status:** Upstream issue acknowledged. The unsoundness relates to internal unsafe code that can lead to undefined behavior under specific access patterns.
- **Disposition:** Replacement planned — tracking upstream fix
- **Conditions:** The unsoundness requires specific concurrent access patterns to the LRU cache that are unlikely in ADK-Rust's single-threaded-per-request usage of tantivy/lancedb.
- **ADK-Specific Context:** The `lru` crate is used internally by tantivy and lancedb for segment caching. ADK-Rust does not directly interact with the `lru` API, and the affected code paths require concurrent mutable access patterns that the upstream libraries do not exercise.
- **Mitigation:** Monitor tantivy/lancedb releases for an update that replaces or upgrades `lru`. Consider switching to an alternative RAG backend if a fix is not forthcoming within two minor releases.

---

### rand 0.7.3 — Outdated

- **Crate:** `rand` (0.7.3)
- **Severity:** Informational (no known security vulnerability)
- **ADK Impact:** Transitive dependency via `adk-auth` → `azure_security_keyvault` → `rand` 0.7.3
- **Status:** The `rand` 0.7.x line is outdated (current stable is 0.8.x+). No security advisory exists, but `cargo audit` flags it as unmaintained/outdated.
- **Disposition:** Accepted risk — no security impact
- **Conditions:** N/A. The outdated version of `rand` has no known security vulnerabilities. The flag is purely informational.
- **ADK-Specific Context:** The `rand` 0.7.3 dependency is pulled in by the Azure SDK crate. ADK-Rust does not depend on `rand` 0.7.3 directly. The Azure SDK team controls when to upgrade their dependency.
- **Mitigation:** No immediate action required. Monitor Azure SDK releases for an upgrade to `rand` 0.8.x. This is a cosmetic `cargo audit` warning with no security impact.

---

## Unmaintained Crates

The following crates are flagged by `cargo audit` as unmaintained. Each has been reviewed for security impact and assigned a disposition.

| Crate | Advisory | Version | Dependency Chain | Disposition | Justification |
|-------|----------|---------|-----------------|-------------|---------------|
| `async-std` | [RUSTSEC-2025-0052](https://rustsec.org/advisories/RUSTSEC-2025-0052) | 1.13.2 | `adk-auth` → `azure_security_keyvault_secrets`; `adk-realtime` → `livekit` → `async-tungstenite` | Acceptable — no security impact | Discontinued runtime, used only as transitive async shim. Will be removed when upstream migrates. |
| `atomic-polyfill` | [RUSTSEC-2023-0089](https://rustsec.org/advisories/RUSTSEC-2023-0089) | 1.0.3 | `adk-rag` → `surrealdb` → `geo-types` → `rstar` → `heapless` | Acceptable — no security impact | Polyfill for non-std targets. ADK-Rust uses std only. |
| `audiopus_sys` | [RUSTSEC-2026-0150](https://rustsec.org/advisories/RUSTSEC-2026-0150) | 0.2.2 | `adk-realtime` → `audiopus` | Replacement planned | Tracking replacement with `opus-rs` or direct `libopus-sys`. |
| `backoff` | [RUSTSEC-2025-0012](https://rustsec.org/advisories/RUSTSEC-2025-0012) | 0.4.0 | `adk-session` → `neo4rs`/`firestore`; `adk-model`/`adk-realtime` → `async-openai` | Acceptable — no security impact | Pure-Rust retry logic. Functional despite unmaintained status. |
| `bincode` | [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141) | 2.0.1 | `adk-rag` → `surrealdb` → `surrealmx` | Acceptable — no security impact | Internal to SurrealDB. No direct ADK usage. |
| `fxhash` | [RUSTSEC-2025-0057](https://rustsec.org/advisories/RUSTSEC-2025-0057) | 0.2.1 | `adk-mistralrs` → `mistralrs` → `bm25` | Acceptable — no security impact | Non-crypto hash for BM25 lookup tables. No security context. |
| `instant` | [RUSTSEC-2024-0384](https://rustsec.org/advisories/RUSTSEC-2024-0384) | 0.1.13 | `adk-auth` → `azure_core` → `http-types` → `futures-lite` → `fastrand` | Acceptable — no security impact | Time shim for non-WASM. Delegates to `std::time::Instant` on native. |
| `number_prefix` | [RUSTSEC-2025-0119](https://rustsec.org/advisories/RUSTSEC-2025-0119) | 0.4.0 | `adk-mistralrs` → `mistralrs` → `indicatif`; `adk-audio` → `tokenizers` → `indicatif` | Acceptable — no security impact | Number formatting for progress bars. No unsafe code. |
| `paste` | [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436) | 1.0.15 | Multiple crates (adk-mistralrs, adk-audio, adk-browser, adk-eval, adk-rag, adk-code, adk-auth, adk-session) | Acceptable — no security impact | Compile-time macro. No runtime behavior. Widely used across Rust ecosystem. |
| `rustls-pemfile` | [RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134) | 1.0.4, 2.2.0 | `adk-auth` → `azure_identity`/`oauth2`/`reqwest`; `adk-rag` → `qdrant-client`; `adk-tool` → `google-cloud-spanner` | Replacement planned | Superseded by built-in `rustls` PEM parser. Will be removed as deps update. |

---

## Disposition Legend

| Disposition | Meaning |
|---|---|
| **Accepted risk** | Vulnerability exists but exploitability conditions make it low-impact for ADK-Rust's usage patterns. No immediate action planned. |
| **Replacement planned** | A fix or replacement is expected upstream. ADK-Rust will upgrade when available. |
| **Upgrade deferred** | A fix exists but cannot be applied yet due to compatibility constraints. Scheduled for a future release. |
| **Resolved** | Advisory has been addressed. Entry retained for audit trail. |

---

## Review Cadence

This document is reviewed at each minor release. Run `cargo audit` to check for new advisories:

```bash
cargo audit
```

To check for outdated dependencies:

```bash
cargo audit --deny warnings
```
