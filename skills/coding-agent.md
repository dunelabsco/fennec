---
name: coding-agent
description: Delegate heavy coding tasks to an external coding CLI (Claude Code, Codex, Aider, OpenCode, or similar). Use for multi-file refactors, feature implementation, or iterative test-driven work.
always: false
---

# coding-agent

For large coding work, delegate to a dedicated coding CLI rather than doing it directly. The CLI has its own tools, context window, and loop — use it when the work would otherwise bloat Fennec's context or exceed its iteration cap.

## When to delegate

- Multi-file refactors whose diff will be dozens of edits.
- "Build this feature" scale work, not "fix this typo".
- Iterative work: read → edit → run tests → edit again.
- Tasks the user has already decided belong to a coding agent.

## When NOT to delegate

- Single-file reads or one-shot edits — Fennec's own file tools are faster.
- Questions about the code that don't require editing — read it and answer directly.
- Work that requires the caller's live conversation context.
- Research-only work — use the `delegate` sub-agent (cheaper, read-only).

## Which tool to invoke

Pick the first option that applies:

1. **`claude_code` tool, if available.** Fennec ships a built-in `claude_code` tool that wraps the `claude` CLI. It handles non-interactive mode, working directory, and error surfacing for you.

   ```
   claude_code(prompt: "<task>", working_dir: "/optional/path")
   ```

2. **Another coding CLI via the shell tool.** If the user has a different CLI installed (Codex, Aider, OpenCode, etc.), invoke it through shell. Confirm the binary exists first:

   ```
   command -v <cli>
   ```

   Then run it non-interactively. Flag shapes differ — do NOT guess. Check `<cli> --help` or the project's docs. Rough patterns seen in the wild:

   ```
   <cli> --print "<task>"
   <cli> exec "<task>"
   echo "<task>" | <cli>
   ```

   Redirect stdout to a file if the task is long, then read it back.

3. **Nothing installed.** Tell the user there is no coding CLI on the host. Do not fall back to doing the large task inline — Fennec's context and iteration cap are the reason you were delegating in the first place.

## Passing context

External coding CLIs do NOT inherit Fennec's conversation. Put everything they need into the task prompt:

- Exact file paths.
- What "done" looks like — "tests passing", "lint clean", "function signatures unchanged", whatever applies.
- Project conventions: commit style, test framework, type-check command.
- Paths the CLI must leave alone.

## After it runs

Verify — do not trust any agent's self-report:

- `git diff` to see what actually changed.
- Run the project's tests yourself via the shell tool; don't rely on the CLI's "I ran the tests" claim.
- Skim the diff for out-of-scope edits.
- If something looks wrong, read the diff before re-prompting.

## Anti-patterns

- Delegating to a coding CLI to answer a question you could answer by reading the file.
- Sending the whole conversation history in the prompt — these CLIs are one-shot, not chat replacements.
- Chaining external agents (one calling another) without a strong reason.
- Reporting the task done to the user without reading the diff yourself.
