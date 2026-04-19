---
name: jira
description: Query and mutate Jira Cloud issues via the REST API v3 — search (JQL), read, create, update, transition. Use for ticket management in Jira-hosted projects. Requires JIRA_BASE_URL, JIRA_EMAIL, and JIRA_API_TOKEN env vars.
always: false
---

# jira

Jira Cloud (hosted on `atlassian.net`) exposes a REST API v3. Auth is HTTP Basic with the user's email + an API token (not the user's password).

## First-time setup

1. Generate an API token: https://id.atlassian.com/manage-profile/security/api-tokens → **Create API token** → name it "Fennec" → copy.
2. Save three env vars:
   ```
   export JIRA_BASE_URL="https://<your-domain>.atlassian.net"
   export JIRA_EMAIL="your-atlassian-account-email@example.com"
   export JIRA_API_TOKEN="..."
   ```

`<your-domain>` is the subdomain your company uses to log in to Jira (e.g. `acme.atlassian.net`).

## Auth header

HTTP Basic with `email:api_token` base64-encoded:

```
Authorization: Basic <base64(email:api_token)>
Accept: application/json
```

At the shell:
```bash
AUTH=$(printf '%s:%s' "$JIRA_EMAIL" "$JIRA_API_TOKEN" | base64)
```

(Not `Bearer`. Jira uses Basic auth for API-token-based access; Bearer is for OAuth, which is a separate, more complex flow reserved for third-party apps.)

## Base URL for API calls

```
$JIRA_BASE_URL/rest/api/3/
```

## Common operations

**Verify the token (current user)**
```
GET /rest/api/3/myself
```

**Search issues (JQL — Jira Query Language)**
```
GET /rest/api/3/search?jql=<url-encoded-jql>&fields=summary,status,assignee&maxResults=20
```

Example JQL strings:
- `assignee = currentUser() AND resolution = Unresolved`
- `project = "ENG" AND status in ("In Progress", "To Do")`
- `updated >= -7d`
- `labels = backend AND sprint in openSprints()`
- `key in (ENG-123, ENG-456)`

URL-encode the JQL when passing as a query string.

**Get one issue**
```
GET /rest/api/3/issue/<issueKey>?fields=summary,description,status,assignee,comment
```

Issue keys look like `ENG-123`. `description` and `comment.body` are returned as Atlassian Document Format (ADF) JSON, not plain text — strip to the text you need.

**Create an issue**
```
POST /rest/api/3/issue
Body:
{
  "fields": {
    "project": {"key": "ENG"},
    "summary": "Short title",
    "description": {
      "type": "doc",
      "version": 1,
      "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Body..."}]}
      ]
    },
    "issuetype": {"name": "Task"}
  }
}
```

**Add a comment**
```
POST /rest/api/3/issue/<issueKey>/comment
Body:
{
  "body": {
    "type": "doc",
    "version": 1,
    "content": [
      {"type": "paragraph", "content": [{"type": "text", "text": "..."}]}
    ]
  }
}
```

**Transition an issue (change status)**

Statuses change via transitions, not direct state writes.

First, list available transitions for this issue:
```
GET /rest/api/3/issue/<issueKey>/transitions
```
Returns `transitions[]` each with `id` + `name`.

Then apply:
```
POST /rest/api/3/issue/<issueKey>/transitions
Body: {"transition": {"id": "<id>"}}
```

**Assign an issue**
```
PUT /rest/api/3/issue/<issueKey>/assignee
Body: {"accountId": "<user-account-id>"}
```

Find account IDs via `GET /rest/api/3/users/search?query=<name-or-email>`.

## ADF — the document format

Jira's `description` and comment bodies use Atlassian Document Format (ADF), a JSON tree. For plain-text round-trips:

- Read: walk `content` recursively, collect `text` fields.
- Write: wrap user text in the minimal shape shown in the Create example above.

For richer formatting (headings, lists, links), look up ADF nodes — but start simple.

## Rules

- Jira is a shared workspace. Creating / transitioning / commenting affects what teammates see. Confirm destructive changes before posting.
- Use JQL for searches rather than paginating through `/search` with no filter — your account can hit thousands of issues.
- `accountId` is the stable user identifier. `displayName` and email can change; don't use them as keys.
- Rate limits: Atlassian publishes limits per tier. Most personal use stays well below them; honour `Retry-After` on 429.

## Failure modes

- `401 Unauthorized` → check the base64 auth header includes `email:token` exactly, and the token is current.
- `403 Forbidden` → token is valid, but the user doesn't have permission on the project.
- `400 Bad Request` with `errors.customfield_...` → Jira's required custom fields aren't filled. The error body names which.
- `404 Not Found` on an issue key → typo, or the issue is in a project the user can't see.
- Empty `issues[]` from JQL search that you know should match → JQL syntax error silently matched nothing. Test the JQL in Jira's web UI first.
