# Coding Agent Examples

Three runnable example crates in [`examples/`](https://github.com/zavora-ai/adk-rust/tree/main/examples),
each a **real agent** (no mocks) that does the work and then **independently
verifies** the result by running the produced code. All default to a Gemini 3
model; set a key first:

```bash
export GOOGLE_API_KEY=…          # or GEMINI_API_KEY
# or: CODING_PROVIDER=openai OPENAI_API_KEY=…
```

---

## `coding_agent`

The harness in action, with four modes:

```bash
# Multi-language demo (Rust, Python, JavaScript) in a temp workspace:
cargo run --manifest-path examples/coding_agent/Cargo.toml

# Build a medium program over a persistent multi-turn session:
cargo run --manifest-path examples/coding_agent/Cargo.toml -- multiturn

# Scenario tour — increasing complexity, each independently verified:
cargo run --manifest-path examples/coding_agent/Cargo.toml -- tour

# A single task in your own directory:
cargo run --manifest-path examples/coding_agent/Cargo.toml -- ./some/dir "make tests pass"
```

- **demo** — one-shot tasks across languages (writes & runs `rustc`/`python3`/`node`).
- **tour** — five scenarios of rising complexity (`hello` → `multifile` →
  `fixtest` → `debug` → `refactor`), each self-verified with a PASS/FAIL summary.
- **multiturn** — one agent/runner/session grows a Python `todo` CLI over five
  turns (add/list → done → rm → robust errors → tests), building on its own prior
  work; verified by running its test suite.

Read it for: the `CodingAgent` harness, the plan→act→observe loop, multi-turn
context retention.

---

## `coding_graph`

The ultra-review [workflow](workflows.md) as an `adk-graph` `StateGraph`:
implement → parallel correctness/edge-case/style reviewers → synthesize (deferred
fan-in) → revise loop → finalize.

```bash
cargo run --manifest-path examples/coding_graph/Cargo.toml
```

Read it for: parallel agents in one super-step, `add_deferred_node_fn` fan-in,
conditional cyclic routing. Verifies by importing the produced function and
asserting the spec + edge cases.

---

## `coding_goal`

Autonomous [`/goal`](cli.md#goal--autonomous-goal-mode-durable-resumable) mode
with **durable checkpointing**: seeds a buggy module + failing test, loops
plan→act→verify until `python3 test_stats.py` passes, prints the persisted
checkpoint, then demonstrates **resume** (a completed goal is a no-op).

```bash
cargo run --manifest-path examples/coding_goal/Cargo.toml
```

Read it for: the verifier-gated goal loop, atomic goal-state checkpointing, and
resume-across-restart.

---

## A suggested path

1. **`coding_agent` (`tour`)** — see the harness solve tasks of rising complexity.
2. **`coding_agent` (`multiturn`)** — watch it build a program over a session.
3. **`coding_goal`** — autonomous, verifier-gated, durable goal mode.
4. **`coding_graph`** — parallel ultra-review orchestration.

The CLI ([`code` / `goal` / `ultracode`](cli.md)) packages these patterns as
first-class commands.

← Back to the [Coding Agent overview](index.md)
