---
name: coding-agent
description: Delegate heavy coding tasks to the `claude_code` tool (external Claude Code CLI). Use for multi-file refactors, feature implementation, or iterative test-driven work.
always: false
---

# coding-agent

Fennec ships a `claude_code` tool that hands a prompt to a locally installed Claude Code CLI. Claude Code has its own tools, context window, and loop — use it when the work is a big enough coding task that Fennec doing it directly would bloat its context or exceed its iteration cap.

## When to reach for claude_code

- Multi-file refactors whose diff will be dozens of edits.
- "Build this feature" scale work, not "fix this typo".
- Iterative work: read → edit → run tests → edit again.
- Tasks the user has already decided belong to a coding agent.

## When NOT to reach for it

- Single-file reads or one-shot edits — Fennec's own file tools are faster.
- Questions about the code that don't require editing — read it and answer.
- Work that requires the caller's live conversation context.
- Research-only work — use the `delegate` sub-agent, which is cheaper and read-only.

## Calling the tool

```
claude_code(
  prompt: "Refactor src/auth.rs to use the new Provider trait. Keep the public API shape. Run `cargo test -p fennec auth` after the edit.",
  working_dir: "/optional/path/to/clone"
)
```

Rules:

- `prompt` is everything Claude Code gets. Put exact paths, "done" criteria, non-obvious constraints, and paths to NOT touch.
- `working_dir` defaults to Fennec's current working directory. Set it when the task lives in a specific clone.
- The tool runs `claude --print <prompt>` non-interactively. Long tasks may appear to hang — that's Claude Code working, not a failure.
- The `claude` binary must be installed and authenticated on the host. If it is not present the tool self-skips or errors cleanly rather than crashing the agent.

## Passing context

Claude Code does NOT inherit Fennec's conversation. Put everything it needs into the `prompt`:

- Exact file paths.
- What "done" looks like — "tests passing", "lint clean", "function signatures unchanged", whatever applies.
- The project's conventions: commit style, test framework, type-check command.
- Paths it must leave alone.

## After it runs

Verify — do not trust the agent's self-report:

- `git diff` to see what actually changed.
- Run the project's tests yourself via the shell tool; don't rely on Claude Code's "I ran the tests" claim.
- Skim the diff for out-of-scope edits.
- If something looks wrong, don't re-prompt blindly — first read what changed and why.

## Anti-patterns

- Using `claude_code` to answer a question you could answer by reading the file.
- Sending it the whole conversation history in the prompt — it is one-shot, not a chat replacement.
- Chaining external agents (one calling another) without a strong reason.
- Reporting the task done to the user without reading the diff yourself.
