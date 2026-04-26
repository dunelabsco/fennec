---
name: google-workspace
description: Interact with Gmail, Google Drive, and Google Calendar via their REST APIs. Use for reading mail, finding Drive files, listing calendar events, creating meetings. Requires OAuth 2.0 — the most involved setup of the bundled skills.
always: false
---

# google-workspace

Gmail, Drive, and Calendar share one auth flow (OAuth 2.0). No single API key is enough — every request carries a short-lived access token minted from the user's Google account.

## First-time setup

Pick Option A if the user is comfortable with a CLI; Option B is the fallback.

### Option A — via `gcloud` (recommended)

1. Install gcloud: https://cloud.google.com/sdk/docs/install.
2. `gcloud auth login` — opens a browser, user signs in.
3. Grant the Workspace scopes:
   ```
   gcloud auth application-default login \
     --scopes=https://www.googleapis.com/auth/gmail.modify,https://www.googleapis.com/auth/drive,https://www.googleapis.com/auth/calendar
   ```
4. Mint an access token on demand:
   ```
   export GOOGLE_ACCESS_TOKEN=$(gcloud auth print-access-token)
   ```
   Tokens expire after about one hour. Re-run when a 401 comes back.

### Option B — manual OAuth client (no gcloud)

Long and finicky. Summarise for the user:

1. Create a Google Cloud project at https://console.cloud.google.com.
2. **APIs & Services → Library** → enable **Gmail API**, **Google Drive API**, **Google Calendar API** — whichever are needed.
3. Configure the **OAuth consent screen**: user type External (or Internal for Workspace orgs), add the user's own email as a Test User.
4. **Credentials → Create Credentials → OAuth client ID** → pick **Desktop app**. Download `client_secret.json`.
5. Run a one-time consent flow (any OAuth library, or `oauth2l`) to get a **refresh token**.
6. Mint an access token from the refresh token as needed:
   ```
   curl -d client_id=<ID> -d client_secret=<SECRET> \
        -d refresh_token=<REFRESH> -d grant_type=refresh_token \
        https://oauth2.googleapis.com/token
   ```
   Parse `access_token` from the JSON response, export it.

Either way, every API request uses:
```
Authorization: Bearer <GOOGLE_ACCESS_TOKEN>
```

If no token is available, **do not guess** — ask the user which option they want and walk them through it.

## Gmail basics

Base: `https://gmail.googleapis.com/gmail/v1/users/me/`

**List recent messages** (IDs only)
```
GET /messages?maxResults=20&q=is:unread
```
`q` accepts Gmail search syntax: `from:`, `to:`, `subject:`, `has:attachment`, `newer_than:7d`, etc.

**Get one message**
```
GET /messages/<id>?format=full
```
`format=metadata` is faster when only headers are needed.

**Send a message**
```
POST /messages/send
Body: {"raw": "<base64url of an RFC 2822 message>"}
```
Build the RFC 2822 string first (`From`, `To`, `Subject`, blank line, body), then `base64url` encode. Use `base64 | tr '+/' '-_' | tr -d '='` at the shell.

## Drive basics

Base: `https://www.googleapis.com/drive/v3/`

**List files**
```
GET /files?q=name contains 'budget' and mimeType='application/pdf'&pageSize=20
```

**Get file metadata**
```
GET /files/<file_id>?fields=id,name,mimeType,modifiedTime,size
```

**Download a file**
```
GET /files/<file_id>?alt=media
```
`?alt=media` is the key — without it you get metadata JSON instead of bytes.

**Upload (simple, < 5 MB)**
```
POST https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart
```
Multipart body: first part is metadata JSON (`name`, `mimeType`), second part is the file bytes. Use `uploadType=resumable` for larger files.

## Calendar basics

Base: `https://www.googleapis.com/calendar/v3/calendars/<calendar_id>/`

Use `primary` as `<calendar_id>` for the user's default calendar.

**List events (one week ahead)**
```
GET /events?timeMin=2026-04-19T00:00:00Z&timeMax=2026-04-26T00:00:00Z&singleEvents=true&orderBy=startTime
```

**Create an event**
```
POST /events
Body:
{
  "summary": "Meeting with Sam",
  "start": {"dateTime": "2026-04-22T15:00:00-04:00"},
  "end":   {"dateTime": "2026-04-22T16:00:00-04:00"}
}
```

**Update an event**
```
PATCH /events/<event_id>
Body: {"summary": "new title"}
```

## Scopes — pick the tightest

| Need | Scope |
|---|---|
| Read Gmail | `https://www.googleapis.com/auth/gmail.readonly` |
| Read + label Gmail | `https://www.googleapis.com/auth/gmail.modify` |
| Send mail | `https://www.googleapis.com/auth/gmail.send` |
| Read Drive | `https://www.googleapis.com/auth/drive.readonly` |
| Read + write Drive | `https://www.googleapis.com/auth/drive` |
| Read Calendar | `https://www.googleapis.com/auth/calendar.readonly` |
| Read + write Calendar | `https://www.googleapis.com/auth/calendar` |

## Failure modes

- `401 Unauthorized` → access token is stale. Mint a new one (`gcloud auth print-access-token`, or refresh-token flow).
- `403 Insufficient Permission` → a needed scope was not granted. Re-run the consent flow including that scope.
- `400 invalid_grant` during refresh → refresh token revoked or expired. Google expires refresh tokens for apps in test mode after ~7 days — either publish the app, or regenerate periodically.
- Drive download returns metadata JSON instead of bytes → missing `?alt=media`.

## Rules

- Access tokens last ~1 hour. Mint fresh on every agent startup; do not persist tokens to disk.
- Never commit a refresh token or `client_secret.json` to git. They are full-account credentials.
- Gmail send: show the user the exact message draft first, wait for explicit go-ahead.
- Drive delete via API is irreversible — always confirm with the user before deleting.
- Calendar creates / updates send invite emails to attendees. Do not create events on shared calendars without showing the user the invitee list first.
