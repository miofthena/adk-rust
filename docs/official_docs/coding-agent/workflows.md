# Graph Workflows (ultra-review)

For anything beyond a single agent loop — parallel reviewers, adversarial checks,
multi-stage pipelines — orchestrate coding agents as an [`adk-graph`](../agents/graph-agents.md)
`StateGraph`. This is how the [`ultracode`](cli.md#ultracode--parallel-ultra-review)
command and the [`coding_graph` example](examples.md#coding_graph) work.

## The "ultra" pattern

```text
  START → implement ──┬─▶ review:correctness ─┐
                      ├─▶ review:edge-cases  ─┤   (parallel, real agents)
                      └─▶ review:style       ─┤
                                               ▼
                                         synthesize   ← deferred fan-in (runs once)
                                               │
                            decision ──────────┤
                        ┌── "revise" ◀──────────┘
                        ▼                       └──▶ "finalize" → END
                     revise ─▶ (back to the reviewers)
```

- **implement / revise** are full [`CodingAgent`](harness.md)s (read/write/bash).
- each **reviewer** is a read-only `LlmAgent` that inspects the code and returns a
  `VERDICT: approve` or `VERDICT: changes` with notes.
- **synthesize** aggregates the verdicts and routes to `revise` or `finalize`,
  bounded by a round cap + the graph recursion limit.

## Fan-out / fan-in in adk-graph

Two `adk-graph` capabilities make this correct:

- **Parallel fan-out** — several nodes reachable from one upstream node run
  **concurrently** in the same super-step.
- **Fan-in barrier** — a node declared with `add_deferred_node_fn` runs **exactly
  once**, only after **all** its upstream paths complete (not once per upstream).

```rust
use adk_graph::graph::StateGraph;
use adk_graph::{DeferredNodeConfig, MergeStrategy};
use adk_graph::edge::{START, END};

let graph = StateGraph::with_channels(&["task", "round", "decision", "notes",
                                        "rev_correctness", "rev_edge-cases", "rev_style"])
    .add_node_fn("implement", /* coding agent */)
    .add_node_fn("review_correctness", /* read-only reviewer */)
    .add_node_fn("review_edge-cases",  /* … */)
    .add_node_fn("review_style",       /* … */)
    // fan-in: runs once, after all three reviewers finish
    .add_deferred_node_fn("synthesize", /* aggregate verdicts */,
        DeferredNodeConfig { merge_strategy: MergeStrategy::Collect, fan_in_timeout: None })
    .add_node_fn("revise", /* coding agent applies notes */)
    .add_node_fn("finalize", /* done */)
    .add_edge(START, "implement")
    .add_edge("implement", "review_correctness")
    .add_edge("implement", "review_edge-cases")
    .add_edge("implement", "review_style")
    .add_edge("revise", "review_correctness")     // loop fans out again
    .add_edge("revise", "review_edge-cases")
    .add_edge("revise", "review_style")
    .add_edge("review_correctness", "synthesize") // fan-in
    .add_edge("review_edge-cases", "synthesize")
    .add_edge("review_style", "synthesize")
    .add_conditional_edges("synthesize",
        |s| s.get("decision").and_then(|v| v.as_str()).unwrap_or("finalize").to_string(),
        [("revise", "revise"), ("finalize", "finalize")])
    .add_edge("finalize", END)
    .compile()?
    .with_recursion_limit(16);
```

> **`add_deferred_node_fn`** (and `mark_deferred` for custom nodes) brings fan-in
> to the core `StateGraph` builder — previously only the higher-level
> `GraphAgentBuilder` could declare a deferred node. Without it, an aggregator
> with multiple upstream edges would fire as soon as the *first* branch finished.

Each node is an `async` closure that runs an agent and writes its result into the
graph state; the conditional edge reads `decision` to loop or finish. See the full,
runnable implementation in the [`coding_graph` example](examples.md#coding_graph)
and the [Graph Agents](../agents/graph-agents.md) reference for the general API.

## When to use a graph vs. the harness

- **Single agent, linear/iterative work** → just a [`CodingAgent`](harness.md)
  (the `code` / `goal` commands).
- **Parallel specialists, multi-stage review, adversarial refutation, swarms** →
  an `adk-graph` workflow (the `ultracode` command).

Next: [Examples →](examples.md)
