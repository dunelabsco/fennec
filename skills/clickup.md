---
name: clickup
description: Query and modify ClickUp tasks, spaces, folders, and lists via the REST API. Use when the user wants to add a task, list what's in a space, mark work complete, or inspect the hierarchy. Requires CLICKUP_API_TOKEN env var.
always: false
---

# clickup

ClickUp's REST API v2 covers the full workspace hierarchy (workspaces → spaces → folders → lists → tasks). Auth is a raw personal API token in the `Authorization` header — **no** `Bearer` prefix (ClickUp quirk, same as Linear).

## First-time setup

1. In ClickUp: click avatar (top-right) → **Settings → Apps → API Token → Generate**.
2. Copy the token. Personal tokens start with `pk_`.
3. Save: `export CLICKUP_API_TOKEN=pk_...`.

## Base URL

```
https://api.clickup.com/api/v2/
```

## Auth header (every request)

```
Authorization: <CLICKUP_API_TOKEN>
Content-Type: application/json
```

**Not `Bearer`.** Personal tokens go bare. OAuth access tokens (from a ClickUp OAuth app) do use `Bearer` — stick with personal tokens for personal / scripted use and keep things simple.

## The hierarchy

ClickUp's structure is deep:
```
Workspace (team)
  └─ Space
      └─ Folder        (optional — lists can live directly in a space)
          └─ List
              └─ Task
                  └─ Subtask
```

Most operations need an ID from one of these levels. Start from the top:

```
GET /api/v2/team                                  # lists workspaces (called "teams" in the API)
GET /api/v2/team/<team_id>/space                  # spaces in a workspace
GET /api/v2/space/<space_id>/folder               # folders in a space
GET /api/v2/space/<space_id>/list                 # lists directly in a space (no folder)
GET /api/v2/folder/<folder_id>/list               # lists in a folder
GET /api/v2/list/<list_id>/task                   # tasks in a list
```

Verify the token works:
```
GET /api/v2/user
```

## Tasks

**List tasks**
```
GET /api/v2/list/<list_id>/task?archived=false&subtasks=true&page=0
```

Useful query params:
- `archived=false` — skip archived.
- `subtasks=true` — include subtasks in the response.
- `statuses[]=open&statuses[]=in+progress` — filter by status name.
- `assignees[]=<user_id>` — filter by assignee.
- `due_date_gt=<ms>` / `due_date_lt=<ms>` — date range (ms since epoch).
- `page=<N>` — zero-indexed pagination, 100 per page.

**Get one task**
```
GET /api/v2/task/<task_id>?custom_task_ids=true&team_id=<team_id>
```

Pass `custom_task_ids=true&team_id=...` if the user's team has custom task IDs enabled (e.g. `ENG-123`) and you're looking one up by the friendly ID.

**Create a task**
```
POST /api/v2/list/<list_id>/task
Body:
{
  "name": "Call dentist",
  "description": "Ask about follow-up x-rays",
  "assignees": [<user_id>],
  "tags": ["personal"],
  "status": "open",
  "priority": 2,
  "due_date": 1714012800000,
  "due_date_time": true
}
```

`due_date` is milliseconds since epoch. `priority`: 1 (urgent) → 4 (low).

**Create a subtask**
```
POST /api/v2/list/<list_id>/task
Body: {"name": "Subtask name", "parent": "<parent_task_id>"}
```
Same endpoint as a normal create — the `parent` field is what makes it a subtask. The `<list_id>` should be the parent task's list (subtasks live under their parent's list).

**Update a task**
```
PUT /api/v2/task/<task_id>
Body: {"name": "new name", "description": "...", "status": "in progress"}
```

**Complete a task** (via status change)
```
PUT /api/v2/task/<task_id>
Body: {"status": "complete"}
```
(Exact status name varies by list; check the list's statuses first.)

**Delete a task** (irreversible)
```
DELETE /api/v2/task/<task_id>
```

**Add a comment**
```
POST /api/v2/task/<task_id>/comment
Body: {"comment_text": "Comment body"}
```

## Custom fields

Many teams use ClickUp custom fields (dropdowns, labels, progress bars, formulas). Listing + setting them:

```
GET /api/v2/list/<list_id>/field                              # field definitions for a list
POST /api/v2/task/<task_id>/field/<field_id>                  # set a field value
Body: {"value": "..."}
```

Field value shape depends on type — dropdowns take the option ID, labels take an array, dates take ms epoch, etc. Fetch field definitions to learn the shape.

## Members, statuses, tags

```
GET /api/v2/team/<team_id>/member                             # users in the workspace
GET /api/v2/list/<list_id>                                    # list metadata, includes statuses
GET /api/v2/space/<space_id>/tag                              # tag definitions
```

## Rules

- ClickUp workspaces are shared — don't mass-create or delete silently. Confirm with the user per write.
- The `Authorization` header is raw, not `Bearer`. Wrong header shape = 401.
- Personal tokens inherit the user's permissions. A read-only user gets a read-only token effectively.
- Timestamps are ms since epoch (not seconds, not ISO). Common source of date bugs.
- Custom task IDs (`ENG-123`) require the `custom_task_ids=true&team_id=...` flag — without it, the API tries to parse the string as a numeric ClickUp ID and 404s.

## Failure modes

- `401 Unauthorized` → token wrong, expired, or (common) you included `Bearer` in the header.
- `OAUTH_019`/`OAUTH_027` errors → OAuth-specific issues on an OAuth token; personal tokens shouldn't hit these.
- `404 Not Found` → ID wrong, or the team uses custom task IDs and you forgot `custom_task_ids=true`.
- `429 Too Many Requests` → rate-limited. ClickUp uses per-token quotas (100 requests / minute on personal tokens). Honour `X-RateLimit-Reset`.
- Response looks wrong (dates off by a day) → ClickUp stores dates in user-local timezone but returns UTC ms. Double-check the conversion.
