# The Coding Harness (`CodingAgent`)

`CodingAgent` is a thin harness — shipped in `adk-agent` behind the `coding`
feature — that wires the [dev tools](devtools.md), a planning `write_todos` tool,
and a minimal coding system prompt onto a normal `LlmAgent`. It's *configuration
over `LlmAgent`*, not a new agent type, so it keeps the default `adk-agent` build
lean (the feature pulls in `adk-devtools`).

## Build one

```rust
use adk_agent::coding::CodingAgent;
use adk_devtools::Workspace;

let coding = CodingAgent::builder()
    .model(model)                              // any adk-model provider (required)
    .workspace(Workspace::new("./my-repo"))    // sandboxed (required)
    .instruction("Follow the project's existing style; prefer small diffs.") // optional, appended
    .build()?;

let agent = coding.agent();   // Arc<dyn Agent> — hand to a Runner
```

Builder options:

| Method | Effect |
|--------|--------|
| `.model(Arc<dyn Llm>)` | The model (required). |
| `.workspace(Workspace)` | The sandboxed workspace (required). |
| `.name("…")` | Agent name (default `"coding-agent"`). |
| `.instruction("…")` | Extra guidance appended to the base coding prompt. |
| `.tool(Arc<dyn Tool>)` | Register an extra tool (MCP, function tool, …). |
| `.without_todos()` | Disable the `write_todos` planning tool. |

## What's wired

- **The dev toolset** — read/write/edit/glob/grep/bash, scoped to the workspace.
- **`write_todos`** — a planning tool the model uses to record and update a short
  task list. Read it back from the harness:

  ```rust
  for todo in coding.todos() {
      println!("[{}] {}", todo.status, todo.content);  // pending | in_progress | completed
  }
  ```

- **A minimal prompt** — sub-1k-token base instructions ("explore before you
  change", "read before you edit", "verify by running tests", "track a plan"),
  kept small on purpose so capabilities come from tools and skills rather than a
  huge prompt.

## The loop

You run a `CodingAgent` like any agent — through a `Runner`. Within a turn the
underlying `LlmAgent` already executes a **plan → act → observe** loop: it calls
tools, sees their results, and continues until it's done. A typical turn:

```text
write_todos(...)            # plan
glob / grep / read_file     # explore
edit_file / write_file      # change
bash("…test…")              # verify
write_todos(... completed)  # update plan
→ final summary
```

For **multi-turn** work, reuse the same `Runner` + session across calls and the
agent builds on its own prior work (it re-reads and edits the files it wrote).
See the [`coding_agent` example](examples.md#coding_agent) (`multiturn` mode).

## Memory & skills

Because it's an `LlmAgent` underneath, everything else in ADK composes:

- Attach a [`MemoryService`](../memory/index.md) (e.g. the bi-temporal knowledge
  graph) on the `Runner` for cross-session project memory.
- Add [lazy skills](../agents/realtime-agents.md) via `adk-skill` to extend
  capabilities without bloating the prompt.
- Hooks/guardrails (`adk-plugin`, `adk-guardrail`) gate tool calls deterministically.

## Running it

The harness produces an `Arc<dyn Agent>`; drive it with `adk-runner` (see the
[quick start](index.md#60-second-quick-start)) or use the
[CLI](cli.md), which wraps all of this.

Next: [The CLI →](cli.md)
