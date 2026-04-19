---
name: google-tasks
description: Read and write Google Tasks (the tasks tool in Gmail / Calendar) via the Tasks API v1. Use when the user wants to add, list, or complete items in Google Tasks. Shares OAuth setup with the google-workspace skill.
always: false
---

# google-tasks

Google Tasks is the lightweight todo app that lives in Gmail / Calendar sidebars and the standalone mobile app. Its API v1 is a simple CRUD REST interface over OAuth 2.0.

## First-time setup

Shares the same OAuth 2.0 flow as the `google-workspace` skill. If `GOOGLE_ACCESS_TOKEN` is already set for Gmail / Drive / Calendar, it works here too — the token just needs the right scope.

Required scopes:

| Need | Scope |
|---|---|
| Read tasks | `https://www.googleapis.com/auth/tasks.readonly` |
| Read + write tasks | `https://www.googleapis.com/auth/tasks` |

Easiest path:
```
gcloud auth application-default login --scopes=https://www.googleapis.com/auth/tasks
export GOOGLE_ACCESS_TOKEN=$(gcloud auth print-access-token)
```

Or add `tasks` to the existing `--scopes` list when setting up the `google-workspace` skill.

See the `google-workspace` skill for the full OAuth setup details.

## Auth header (every request)

```
Authorization: Bearer <GOOGLE_ACCESS_TOKEN>
```

Tokens last ~1 hour. On 401, mint a fresh one via `gcloud auth print-access-token`.

## Base URL

```
https://tasks.googleapis.com/tasks/v1/
```

## Task lists (the top-level containers)

Google Tasks organises items into lists. Most users have a single "My Tasks" list; power users have several.

**List the user's task lists**
```
GET /tasks/v1/users/@me/lists
```

Returns `items[]` with `id`, `title`, `updated`.

**Create a list**
```
POST /tasks/v1/users/@me/lists
Body: {"title": "Work"}
```

## Tasks

**List tasks in a list**
```
GET /tasks/v1/lists/<tasklist_id>/tasks?showCompleted=false&maxResults=100
```

Key query params:
- `showCompleted=true/false` — default true; often false is what you want for "what's open".
- `showHidden=false` — exclude archived recurring tasks.
- `dueMin` / `dueMax` — ISO 8601 range filter.

**Get one task**
```
GET /tasks/v1/lists/<tasklist_id>/tasks/<task_id>
```

**Create a task**
```
POST /tasks/v1/lists/<tasklist_id>/tasks
Body:
{
  "title": "Call dentist",
  "notes": "Ask about follow-up",
  "due": "2026-04-24T00:00:00Z"
}
```

`due` is ISO 8601 UTC but Google Tasks treats it as a **date only** — the time portion is ignored. All-day only; Google Tasks has no time-of-day support.

**Update a task**
```
PATCH /tasks/v1/lists/<tasklist_id>/tasks/<task_id>
Body: {"title": "new title", "notes": "new notes"}
```

**Mark complete**
```
PATCH /tasks/v1/lists/<tasklist_id>/tasks/<task_id>
Body: {"status": "completed"}
```

`status` values: `needsAction` (default) or `completed`. When `status: completed`, Google auto-sets `completed: "<current-time>"`.

**Reopen**
```
PATCH /tasks/v1/lists/<tasklist_id>/tasks/<task_id>
Body: {"status": "needsAction", "completed": null}
```

**Delete**
```
DELETE /tasks/v1/lists/<tasklist_id>/tasks/<task_id>
```

## Subtasks

A task becomes a subtask by setting its `parent` to another task's ID within the same list. Move a task under a parent:
```
POST /tasks/v1/lists/<tasklist_id>/tasks/<task_id>/move?parent=<parent_task_id>
```

Move to top level:
```
POST /tasks/v1/lists/<tasklist_id>/tasks/<task_id>/move
```

Move to a specific position under a parent:
```
POST /tasks/v1/lists/<tasklist_id>/tasks/<task_id>/move?parent=<parent>&previous=<previous_sibling_id>
```

## Rules

- Google Tasks has no label / tag system and no time-of-day dues. For richer task management, route the user to Todoist (`todoist` skill) or Linear.
- `due` is a date; the time portion is ignored. Don't mislead the user by showing a specific hour.
- Deletions are soft (marked `deleted: true`, filtered out by default) and can be undone for 30 days via `showDeleted=true`. Good safety net.
- Tasks don't have owners or assignees — they're always the authenticated user's. Don't try to assign to others.

## Failure modes

- `401 Unauthorized` → token stale, or the token doesn't include a tasks scope.
- `403 Insufficient Permission` → scope is `tasks.readonly` but you tried to write. Re-consent with `tasks` scope.
- `404 Not Found` on a task list → the user deleted it, or the ID is from a different account.
- `400 Invalid Value` on `due` → pass a valid ISO 8601 UTC string; don't send just `"2026-04-24"`.
