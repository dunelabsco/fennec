---
name: things-mac
description: Create and update tasks in Things 3 on macOS via its URL scheme. Use when the user runs Cultured Code's Things app and wants to capture something without opening the UI. macOS only.
always: false
---

# things-mac

Things 3 supports a rich URL-scheme API (`things:///`) for creating and modifying tasks. Invoked via macOS's `open` command. No HTTP, no API key — just URL parameters. macOS only.

If the user doesn't have Things 3 installed, the scheme won't resolve. The skill's failure modes cover that case.

## First-time setup

No external setup for creating tasks. For **modifying** existing tasks (update, complete, cancel), Things requires an auth token as a safeguard against malicious links:

1. In Things: **Things menu → Settings → General → Enable Things URLs**.
2. Click **Manage** → copy the auth token.
3. Save (only needed for mutations): `export THINGS_TOKEN=...`.

Read / create works without a token.

## Invocation

```
open "things:///<command>?<params>"
```

URL-encode special characters in parameters (`%20` for spaces, `%23` for `#`, `%2C` for `,`, etc.).

## Add a task

```
open "things:///add?title=Call%20dentist&when=today&notes=Ask%20about%20follow-up&tags=personal,health"
```

Key `add` params:
- `title` — required, task name.
- `notes` — longer description.
- `when` — `today`, `tomorrow`, `evening`, `anytime`, `someday`, or an ISO date `2026-04-24`.
- `deadline` — ISO date `2026-04-24` for a hard due date.
- `tags` — comma-separated (create tags on the fly if they don't exist).
- `list` / `list-id` — drop into a specific project or area.
- `heading` / `heading-id` — drop under a specific heading inside a project.
- `checklist-items` — newline-separated items (URL-encode `\n` as `%0A`).
- `completed` / `canceled` — `true` to create-as-done.
- `reveal` — `true` to open Things and focus the new task.

## Add a project

```
open "things:///add-project?title=New%20project&area=Work&to-dos=Step%201%0AStep%202%0AStep%203"
```

Additional params:
- `to-dos` — newline-separated initial tasks.
- `area` / `area-id` — which area to file under.

## Show / navigate

```
open "things:///show?id=<task-or-project-id>"
open "things:///show?query=Inbox"                  # show a built-in list (Inbox, Today, Upcoming, Anytime, Someday, Logbook)
```

## Search

```
open "things:///search?query=dentist"
```

Opens the Things search UI with the query prefilled.

## Update an existing task (needs auth token)

```
open "things:///update?id=<task_id>&auth-token=<THINGS_TOKEN>&title=New%20title&completed=true"
```

Without `auth-token`, the URL silently fails (Things protects data modifications). Updateable fields: `title`, `notes`, `tags`, `when`, `deadline`, `completed`, `canceled`, `list`, `heading`, `prepend-notes`, `append-notes`.

## JSON API (bulk create)

For creating multiple tasks + projects at once, Things accepts a JSON array as the `data` parameter:

```
open "things:///json?data=<url-encoded-json-array>"
```

Shape:
```json
[
  {
    "type": "to-do",
    "attributes": {
      "title": "Task 1",
      "when": "today",
      "tags": ["work"]
    }
  },
  {
    "type": "project",
    "attributes": {
      "title": "New project",
      "area": "Work",
      "items": [
        {"type": "to-do", "attributes": {"title": "Step 1"}}
      ]
    }
  }
]
```

The JSON payload is the only way to create a project with nested tasks atomically. Remember to URL-encode the whole thing.

## Finding IDs

Things doesn't expose a "list all tasks" command via URL. To get a task's ID:
1. In the Things app, right-click a task → **Share → Copy Link**. That link is the `things:///show?id=<id>` URL.
2. Or `Edit → Copy Link`. Paste and extract the ID.

For scripting that needs real "list / query" access, AppleScript is the fallback — Things 3 has a full AppleScript dictionary. Out of scope for this skill, but `osascript` can enumerate tasks and get IDs programmatically.

## Rules

- **URL-encode every parameter value.** Un-encoded `&` in a title silently splits params; un-encoded `#` truncates the URL.
- Things URLs are fire-and-forget. The `open` command returns immediately; you can't tell if the task was actually created from the shell exit code.
- For anything that really needs a confirmation, use the AppleScript route instead.
- Mutations need `auth-token`. Don't commit that token to git; it's a shared secret.
- Create-with-`completed=true` records the task as done in the Logbook but bypasses the normal completion animation.

## Failure modes

- Nothing happens on `open "things:///..."` → Things isn't installed, or the Things URL scheme is disabled (Settings → General → Enable Things URLs). Verify via `open "things:///version"`, which returns the app + scheme version if it's wired up.
- Task created but fields missing → a parameter wasn't URL-encoded and split into pieces. Test with a minimal URL first.
- Update silently no-ops → missing `auth-token`, or wrong task ID.
- JSON API rejects input → malformed JSON; the payload must be a top-level array, each item a `{type, attributes}` pair.
