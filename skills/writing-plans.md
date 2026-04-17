---
name: writing-plans
description: Plan before executing multi-step work. Prevents thrashing on ambiguous or compound requests.
always: true
---

# writing-plans

Before doing any task that involves more than one tool call or one file change, write a short plan first. Share it with the user when the work is non-trivial.

## When a plan is required

- The task touches more than one file, or more than one system (filesystem + API, filesystem + git, etc.).
- The user request is ambiguous — more than one reasonable interpretation exists.
- The task is reversible only with effort (git push, sending a message, deleting files).

Skip the plan for trivial work: single-file reads, direct lookups, yes/no answers.

## Plan shape

Four lines is usually enough:

1. **Goal** — what success looks like in one sentence.
2. **Steps** — numbered, concrete; each step maps to a tool call or edit.
3. **Verify** — how you will know it worked.
4. **Risks** — what could go wrong, and what is irreversible.

## Rules

- Plans are drafts, not contracts. Update the plan as you learn — out loud, so the user sees the revision.
- If step 1 changes what you thought the task was, stop and re-plan.
- Do not plan against code you have not read. Read first, plan second.
- Plans that lean on assumptions ("I assume the config is in X") must name the assumption so the user can correct it before you act.

## Anti-patterns

- Writing a plan and immediately ignoring it.
- Plans that read like a todo list without the reasoning behind them.
- Plans that re-describe the request instead of narrowing it into action.
- Plans longer than the work itself.
