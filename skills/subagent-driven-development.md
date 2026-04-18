---
name: subagent-driven-development
description: Delegate bounded research tasks to a read-only sub-agent via the `delegate` tool. Use for codebase sweeps, parallel investigations, and context-bloating lookups.
always: false
---

# subagent-driven-development

Fennec's `delegate` tool spawns a fresh sub-agent with a read-only toolkit (read_file, list_dir, web_fetch, web_search), runs it synchronously, and returns one result. Use it to keep the main context clean during research.

## When to delegate

- **Bounded research.** "List every call site of `Foo::bar` and summarise what each caller expects back." The sub-agent reads; you read the summary.
- **Parallel sweeps.** Multiple `delegate` calls in one turn, one per topic — as long as the tasks don't share state.
- **Context protection.** Raw file contents or long tool outputs would blow out the main context, but you only need the distilled answer.
- **Focused doc review.** "Read this URL and tell me whether it confirms our X assumption."

## When NOT to delegate

- The task requires the caller's full conversation context. Sub-agents start fresh; they will miss nuance.
- The task is one tool call. Just make the call.
- The task's output IS the synthesis — delegating "decide what to do next" wastes the sub-agent. That judgement belongs to the caller.
- The task needs writes or side effects. The sub-agent is read-only by design. For coding handoffs, use the `claude_code` tool. For API calls, keep it in the main agent.
- You need steering mid-run. Sub-agents run to completion with no dialogue.

## How to call it

The tool is `delegate`:

```
delegate(
  task: "Read src/memory/sqlite.rs and list every public method on SqliteMemory with a one-line description of what each returns.",
  tools: ["read_file", "list_dir"]     // optional; defaults to the sub-agent's full read-only toolkit
)
```

Rules:

- The `task` string is all the context the sub-agent gets. Be explicit about paths, inputs, and the deliverable shape.
- Restrict `tools` to the minimum needed — faster loop, less drift.
- The sub-agent is capped at 10 tool iterations. Tasks requiring dozens of calls don't fit — break them into multiple delegations.

## Writing a good task string

Treat it as a memo to a new engineer on day one, no context of the current conversation:

> Read `src/collective/search.rs` and `src/collective/cache.rs`. For each public function, list its name, argument types, return type, and a one-sentence purpose. Return a markdown table only — no prose.

Bad:

> Look at the collective code and tell me what's there.

## After it returns

- The sub-agent's result is text. Treat it as research notes, not ground truth — cite it, don't paste it into a user-facing reply verbatim.
- If the result reveals the task was wrong, re-delegate with a corrected task. Don't keep mining a stale output.

## Anti-patterns

- Asking the sub-agent to make a decision that belongs to the caller.
- Nesting sub-agents (sub-agent delegating again). Depth cap and cost add up; the top-level agent should orchestrate.
- Long-running coordination tasks — if it needs three rounds of "then go do X", you wanted a plan, not a delegation.
- Passing write-tool names in the `tools` array. The sub-agent's registered toolkit is read-only for safety; invented tool names will be rejected.
