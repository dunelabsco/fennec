---
name: asana
description: Read and write Asana tasks, projects, and workspaces via the REST API. Use when the user wants to add a task, list what's on a project, mark work complete, or inspect workspaces. Requires ASANA_ACCESS_TOKEN env var.
always: false
---

# asana

Asana's REST API covers tasks, projects, workspaces, teams, and users. Auth is simple Bearer token; for personal / scripted use, a Personal Access Token (PAT) is the shortest path.

## First-time setup

1. Sign in to Asana → click profile (top-right) → **Settings → Apps → Developer Apps**.
2. Scroll to **Personal access tokens** → **Create new token**. Name it (e.g. "Fennec").
3. Copy the token — shown once.
4. Save: `export ASANA_ACCESS_TOKEN=...`.

PATs inherit the permissions of the user who creates them — whatever the user can see and do in Asana, the token can.

## Base URL

```
https://app.asana.com/api/1.0/
```

## Auth header (every request)

```
Authorization: Bearer <ASANA_ACCESS_TOKEN>
Accept: application/json
```

## Verify the token

```
GET /api/1.0/users/me
```

Returns the authenticated user + their workspaces. Use as a smoke test.

## Workspaces

Most work happens inside a workspace. A user may belong to several.

```
GET /api/1.0/workspaces
GET /api/1.0/workspaces/<workspace_gid>
```

Asana IDs are called "gids" (short for "global id"); they're numeric strings.

## Tasks

**List tasks assigned to me in a workspace**
```
GET /api/1.0/tasks?assignee=me&workspace=<workspace_gid>&completed_since=now&opt_fields=name,due_on,projects,completed&limit=50
```

- `assignee=me` — shortcut for the authenticated user.
- `completed_since=now` — filter to uncompleted tasks only (quirk of Asana's filter design: "now" means "nothing completed since now" = only incomplete).
- `opt_fields=` — Asana returns minimal fields by default; name the ones you need.

**Tasks in a project**
```
GET /api/1.0/projects/<project_gid>/tasks?opt_fields=name,assignee,due_on,completed&limit=100
```

**One task**
```
GET /api/1.0/tasks/<task_gid>?opt_fields=name,notes,due_on,due_at,assignee,projects,tags,parent
```

**Create a task**
```
POST /api/1.0/tasks
Content-Type: application/json

{
  "data": {
    "name": "Call dentist",
    "notes": "Ask about follow-up x-rays",
    "workspace": "<workspace_gid>",
    "projects": ["<project_gid>"],
    "assignee": "me",
    "due_on": "2026-04-24",
    "due_at": "2026-04-24T15:00:00Z"
  }
}
```

Asana wraps every request and response body in a `{ "data": ... }` envelope. Don't forget the outer `data` key on writes.

`due_on` is date-only; `due_at` is a datetime. Send only one (whichever matches the task's granularity).

**Update a task**
```
PUT /api/1.0/tasks/<task_gid>
Body: {"data": {"name": "new name", "notes": "new notes"}}
```

**Mark complete**
```
PUT /api/1.0/tasks/<task_gid>
Body: {"data": {"completed": true}}
```

**Add task to a project**
```
POST /api/1.0/tasks/<task_gid>/addProject
Body: {"data": {"project": "<project_gid>"}}
```

**Add a comment ("story")**
```
POST /api/1.0/tasks/<task_gid>/stories
Body: {"data": {"text": "Comment body"}}
```

## Projects

```
GET /api/1.0/workspaces/<workspace_gid>/projects?opt_fields=name,archived,owner
GET /api/1.0/projects/<project_gid>
POST /api/1.0/projects     # body: {"data": {"name": "...", "workspace": "<gid>"}}
```

## Sections (columns inside a project)

```
GET /api/1.0/projects/<project_gid>/sections
POST /api/1.0/tasks/<task_gid>/addProject
Body: {"data": {"project": "<project_gid>", "section": "<section_gid>"}}
```

## Search

Workspace-scoped typeahead (for autocomplete-like lookups):
```
GET /api/1.0/workspaces/<workspace_gid>/typeahead?resource_type=task&query=<term>&count=20
```

`resource_type`: `task`, `project`, `user`, `tag`, `portfolio`.

## Tips

- `opt_fields` is the difference between a 200 KB and a 5 MB response. Always specify.
- Bulk operations (multiple tasks in one request): Asana doesn't have a generic batch API; use the "Batch API" endpoint (`POST /api/1.0/batch`) which wraps up to 10 sub-requests.
- Pagination: responses with many items include `next_page.offset`. Pass back as `offset=` on the next request. Also respects `limit=` up to 100.
- Dates in Asana are timezone-naive `YYYY-MM-DD` unless you use `due_at` / `start_at` (full ISO 8601).

## Rules

- Asana workspaces may be shared (company / team). Don't mass-create or delete tasks silently — confirm each write with the user.
- PATs leak like any other token — don't commit, don't paste into gists, don't put in memory entries.
- Sections aren't required; tasks can live at project root. If the user's workflow uses sections (Kanban), put new tasks in the right one.
- Rate limits: Asana enforces per-user quotas but they're generous for normal use. On 429, respect `Retry-After`.

## Failure modes

- `401 Not Authorized` → token wrong or revoked.
- `403 Forbidden` → token valid but the user doesn't have access to the resource.
- `404 Not Found` → wrong gid, or the object was deleted.
- `400 Invalid Request` with `errors[].message` → field validation error. Read the message; usually a missing required field or a wrong field type.
- Response `errors` array even on 200 → partial success on batch calls; check each sub-response.
