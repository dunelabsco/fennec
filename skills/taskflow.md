---
name: taskflow
description: Manage multi-step work with Fennec's `todo` tool and memory tools. Use when a task has more than two or three steps or spans more than one turn.
always: false
---

# taskflow

Fennec has two complementary task stores. Pick the right one per piece of work.

- **`todo` tool** — ephemeral, in-memory list. One per session. Statuses: pending / in_progress / completed / cancelled. Use for the current piece of work.
- **Memory tools** (`memory_store`, `memory_recall`, `memory_forget`) — persistent across sessions. Use for commitments, deadlines, and open loops the user cares about next week.

## When to use `todo`

- The task has more than ~2 discrete steps.
- The work spans more than one turn of the conversation.
- You want to show the user what's next and what's done.

Skip the todo list for single-step asks. It is noise.

## When to use memory instead

- The user mentions a future commitment ("remind me next Monday", "I'm meeting Sam Thursday").
- A recurring preference or decision ("always use pytest-xdist on this project").
- Project context that should survive a restart.

For time-based reminders that should ping the user back, use the `cron` tool — not `todo`, not memory.

## Triage pattern

When a new request arrives, place each piece into one of five buckets:

1. **Now** — do it this turn. `todo` item, status `in_progress`.
2. **Next** — do it this session but after the current step. `todo` item, status `pending`.
3. **Later** — outside this session; belongs in memory with a date or trigger, or in cron with a schedule.
4. **Delegate** — hand off to the `delegate` tool (research) or `claude_code` tool (coding).
5. **Drop** — not worth doing. Say so to the user; don't silently discard.

## Keeping the list tidy

- Flip to `in_progress` when you start an item. Back to `pending` is rare — it usually means you shouldn't have started.
- Flip to `completed` the moment a step is done, not at the end of the whole task.
- Mark `cancelled` when a step becomes irrelevant. Don't let stale items hang around.
- The list is per-session. Before ending a session with unfinished items, move anything still meaningful into memory.

## Anti-patterns

- One-item todo lists. Overhead with no benefit.
- Todos that re-describe the user's question without narrowing it into action.
- Leaving items `pending` that you never intend to do — mark them `cancelled` so the state is honest.
- Using `todo` for things the user asked you to remember tomorrow. That is memory's or cron's job.
