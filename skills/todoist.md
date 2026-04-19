---
name: todoist
description: Query and modify Todoist tasks and projects via the REST API. Use when the user wants to add a task, list upcoming work, complete items, or reorganise projects. Requires TODOIST_API_TOKEN env var.
always: false
---

# todoist

Todoist exposes a REST API with token auth. Free and Pro accounts both get full API access.

## First-time setup

1. In Todoist: **Settings → Integrations → Developer** → copy the **API token**.
2. Save: `export TODOIST_API_TOKEN=...`.

If the token is missing, ask the user to complete setup. Never fabricate tasks.

## Base URL

```
https://api.todoist.com/api/v1
```

## Auth header (every request)

```
Authorization: Bearer <TODOIST_API_TOKEN>
```

## Common operations

**List all active (not-completed) tasks**
```
GET /api/v1/tasks
```

**Filter tasks by project / label / due date**
```
GET /api/v1/tasks?project_id=<id>
GET /api/v1/tasks?label=<name>
GET /api/v1/tasks?filter=today
GET /api/v1/tasks?filter=overdue
GET /api/v1/tasks?filter=7+days
```

The `filter` param accepts Todoist's full filter syntax (`p:ProjectName`, `@label`, `overdue`, `due: today`, combined with `&` and `|`).

**Get one task**
```
GET /api/v1/tasks/<id>
```

**Create a task**
```
POST /api/v1/tasks
Body: {"content": "Call dentist", "due_string": "tomorrow at 3pm", "priority": 2}
```

- `content` — required. Task title.
- `description` — optional, longer body.
- `project_id` — where to put it; omit for Inbox.
- `section_id` — optional, inside a project.
- `parent_id` — sub-task.
- `priority` — 1 (p4, default) to 4 (p1, urgent). Todoist inverts the numbers confusingly.
- `due_string` — natural language ("tomorrow 3pm", "every weekday", "Friday"). Todoist's parser is excellent.
- `due_date` — explicit `YYYY-MM-DD` (all-day).
- `labels` — array of label names.

**Update a task**
```
POST /api/v1/tasks/<id>
Body: {"content": "Call dentist — reschedule"}
```

**Complete (close) a task**
```
POST /api/v1/tasks/<id>/close
```

Response is `204 No Content` on success.

**Reopen a completed task**
```
POST /api/v1/tasks/<id>/reopen
```

**Delete**
```
DELETE /api/v1/tasks/<id>
```

## Projects

```
GET /api/v1/projects
GET /api/v1/projects/<id>
POST /api/v1/projects           # {"name": "..."}
DELETE /api/v1/projects/<id>
```

## Labels

```
GET /api/v1/labels
POST /api/v1/labels             # {"name": "waiting-for"}
```

## Filters, priority, natural language

Todoist's strength is its natural-language parser. For a task with the phrase `"every Monday at 9am"`, pass that literal string as `due_string` — the parser figures out the recurrence. Works for most human phrasings.

Priority mapping (API number → UI label):
- 4 → p1 (red, urgent)
- 3 → p2 (orange, important)
- 2 → p3 (blue, normal)
- 1 → p4 (no flag, default)

## Rules

- Show the user the task you're about to create before calling POST — especially with due dates, since misparses happen.
- Bulk operations: use separate requests; Todoist's REST API doesn't have a true batch endpoint (use the sync API for that — out of scope here).
- Recurring tasks: setting `due_string: "every Monday"` creates a recurring task. Closing it advances to the next occurrence — don't delete recurring tasks accidentally thinking you're "finishing for today".

## Failure modes

- `401 Unauthorized` → token wrong.
- `403 Forbidden` → the token's account doesn't own the `project_id` you're writing to.
- `400 Bad Request` with parser error → `due_string` wasn't understood. Fall back to `due_date` in ISO format.
- `404` on a closed task → the task was completed / deleted; fetch with `GET /tasks/<id>` to see if it exists.
- Tasks created without `project_id` go to Inbox — confirm that's what the user expected.
