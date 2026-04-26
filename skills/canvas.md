---
name: canvas
description: Read the user's Canvas LMS — courses, assignments, announcements, upcoming events. Use for "what's due this week", syllabus lookups, submitting text entries. Requires CANVAS_BASE_URL and CANVAS_API_TOKEN env vars.
always: false
---

# canvas

Canvas LMS is used by thousands of institutions, each with its own instance URL. The REST API shape is identical across instances — only the base URL differs.

## First-time setup (user does this once)

1. Ask the user for their Canvas instance URL. Typical shapes:
   - `https://canvas.<school>.edu`
   - `https://<school>.instructure.com`
2. Tell them how to generate a token:
   - Canvas → **Account → Settings** → scroll to **Approved Integrations** → click **+ New Access Token**.
   - Name it (e.g. "Fennec"), leave expiry blank or pick one, click **Generate Token**.
   - Copy the token — shown once.
3. Save both values:
   ```
   export CANVAS_BASE_URL="https://canvas.myschool.edu"
   export CANVAS_API_TOKEN="..."
   ```

If either env var is missing, ask the user to complete setup. Do not guess an institution.

## Auth (every request)

```
Authorization: Bearer <CANVAS_API_TOKEN>
```

All responses are JSON. All access is HTTPS against the user's own Canvas domain.

## Common operations

**Verify the token** (current user profile)
```
GET $CANVAS_BASE_URL/api/v1/users/self
```

**List enrolled courses**
```
GET $CANVAS_BASE_URL/api/v1/courses?enrollment_state=active&per_page=50
```

**Assignments in a course**
```
GET $CANVAS_BASE_URL/api/v1/courses/<course_id>/assignments?per_page=100
```

**Upcoming items (due dates across all courses)**
```
GET $CANVAS_BASE_URL/api/v1/users/self/upcoming_events
```

**Todo list**
```
GET $CANVAS_BASE_URL/api/v1/users/self/todo
```

**Announcements across specific courses**
```
GET $CANVAS_BASE_URL/api/v1/announcements?context_codes[]=course_<id1>&context_codes[]=course_<id2>&start_date=2026-01-01
```

**Submit a text-entry assignment**
```
POST $CANVAS_BASE_URL/api/v1/courses/<course_id>/assignments/<assignment_id>/submissions
Body: {"submission": {"submission_type": "online_text_entry", "body": "<html-ish text>"}}
```

## Pagination

Canvas paginates with the `Link` HTTP header, **not** a `next_page` field in the body:
```
Link: <...?page=2&per_page=50>; rel="next", <...?page=10>; rel="last"
```
Follow `rel="next"` until it's absent. Always pass `per_page=50` (or 100 on endpoints that accept it) to reduce round-trips.

## Failure modes

- `401 Unauthorized` → bad or expired token. User generates a new one.
- `403 Forbidden` → token is valid but lacks permission for that resource (e.g. a teacher-only endpoint the user can't see).
- `404 Not Found` → course/assignment ID wrong, or the user isn't enrolled in that course.
- Trailing slash on `CANVAS_BASE_URL` breaks URL concatenation — strip it before appending `/api/v1/...`.

## Rules

- Treat `CANVAS_BASE_URL` as user data. Every user has their own school — never assume, never hardcode.
- Never submit an assignment silently. Show the user the exact text you're about to submit and wait for confirmation.
- Grades endpoints return numerical scores — don't paste those into a chat without the user asking.
- Store no tokens in logs or memory entries. If a token leaks, the user needs to revoke it via Canvas settings.
