---
name: bluesky
description: Post to Bluesky, read the user's timeline, search, and interact via the AT Protocol. Use when the user wants to publish a skeet, check their feed, or follow accounts. Requires BLUESKY_HANDLE + BLUESKY_APP_PASSWORD env vars.
always: false
---

# bluesky

Bluesky runs on the AT Protocol (ATProto). Auth uses **app passwords** — NOT the user's main Bluesky password. App passwords are generated in-app, scoped, and revocable without affecting the main account.

## First-time setup

1. In the Bluesky app or at https://bsky.app: **Settings → App Passwords → Add App Password**.
2. Name it (e.g. "Fennec"), copy the generated password (shown once, format: `xxxx-xxxx-xxxx-xxxx`).
3. Save:
   ```
   export BLUESKY_HANDLE="yourname.bsky.social"      # or custom domain handle
   export BLUESKY_APP_PASSWORD="xxxx-xxxx-xxxx-xxxx"
   ```

**Do not use the main account password.** App passwords have limited scope and can be revoked individually.

## Session handshake

AT Protocol requires minting a session JWT before each batch of requests. Sessions last ~2 hours; treat like a short-lived access token.

```
POST https://bsky.social/xrpc/com.atproto.server.createSession
Content-Type: application/json
Body: {"identifier": "<BLUESKY_HANDLE>", "password": "<BLUESKY_APP_PASSWORD>"}
```

Response:
```json
{
  "did": "did:plc:...",
  "accessJwt": "...",
  "refreshJwt": "...",
  "handle": "yourname.bsky.social"
}
```

Store `accessJwt` and `did` for the duration of this batch. Use `refreshJwt` on `com.atproto.server.refreshSession` if `accessJwt` expires.

## Auth header (after createSession)

```
Authorization: Bearer <accessJwt>
```

## Create a post

```
POST https://bsky.social/xrpc/com.atproto.repo.createRecord
Content-Type: application/json
Body:
{
  "repo": "<did>",
  "collection": "app.bsky.feed.post",
  "record": {
    "$type": "app.bsky.feed.post",
    "text": "hello from fennec",
    "createdAt": "2026-04-19T12:00:00Z"
  }
}
```

`createdAt` must be ISO 8601 UTC. Omit it and the post loses its timestamp.

Reply to another post:
```json
"record": {
  "$type": "app.bsky.feed.post",
  "text": "...",
  "createdAt": "...",
  "reply": {
    "root": {"uri": "<root_post_uri>", "cid": "<root_post_cid>"},
    "parent": {"uri": "<parent_post_uri>", "cid": "<parent_post_cid>"}
  }
}
```

The post's `uri` + `cid` are visible in the parent's raw record (or via `getPosts`).

## Read timeline

**Home timeline**
```
GET https://bsky.social/xrpc/app.bsky.feed.getTimeline?limit=20
```

**A specific user's feed**
```
GET https://bsky.social/xrpc/app.bsky.feed.getAuthorFeed?actor=<handle>&limit=20
```

**Search posts**
```
GET https://bsky.social/xrpc/app.bsky.feed.searchPosts?q=<query>&limit=10
```

## Delete a post

```
POST https://bsky.social/xrpc/com.atproto.repo.deleteRecord
Body:
{
  "repo": "<did>",
  "collection": "app.bsky.feed.post",
  "rkey": "<record-key-from-uri-last-segment>"
}
```

The `rkey` is the last path segment of the post's `uri` (`at://did:plc:.../app.bsky.feed.post/<rkey>`).

## Character limits & quirks

- Posts: 300 graphemes (not bytes or characters — emoji count as 1).
- Facets (mentions, links, hashtags) must be pre-annotated with byte offsets. For plain text, skip facets; the Bluesky client renders links and hashtags automatically for you.
- To embed a link with a custom display, send `text` with the visible string and a `facets` array naming the byte range + URI.
- Image uploads: two-step — upload blob via `com.atproto.repo.uploadBlob`, then reference in the post record.

## Rules

- **Confirm before posting.** Skeets are public, federate across the ATProto network, and Bluesky's search is aggressive.
- Session tokens expire — detect `ExpiredToken` error and call refreshSession rather than re-running createSession every time.
- Bluesky's PDS architecture means a user's data lives on their PDS, not the central server. For most users, the PDS is `bsky.social`; custom PDS setups require pointing at their URL instead.
- The `$type` field on records is mandatory and must match the collection.

## Failure modes

- `AuthenticationRequired` → session expired or wrong app password. Re-authenticate.
- `InvalidRequest` with `Record/text must be a string` → body shape wrong; check JSON structure.
- `RateLimitExceeded` → back off. Bluesky's rate limits are generous for personal use.
- Post appears in the API but not on the web → federation lag; wait a minute. If still invisible, the PDS may be having issues.
