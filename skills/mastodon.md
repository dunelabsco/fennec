---
name: mastodon
description: Post statuses, read timelines, and search on the user's Mastodon instance via its API. Use when the user wants to publish a toot, check their home timeline, or search the fediverse. Requires an access token in MASTODON_ACCESS_TOKEN and the instance URL in MASTODON_INSTANCE_URL.
always: false
---

# mastodon

Mastodon is federated: every user picks an instance (mastodon.social, hachyderm.io, fosstodon.org, etc.). The API is the same, but the base URL differs per instance.

## First-time setup

Two paths. **Option A** is by far the simpler one for personal use.

### Option A — Preferences → Development (recommended)

1. Go to the user's Mastodon instance → **Preferences → Development → New application**.
2. Set:
   - **Application name**: e.g. "Fennec".
   - **Scopes**: `read write` (or `read write follow push` for full access).
3. Create the application. It issues an **access token** immediately (no OAuth dance).
4. Save both values:
   ```
   export MASTODON_INSTANCE_URL="https://mastodon.social"
   export MASTODON_ACCESS_TOKEN="..."
   ```

### Option B — full OAuth 2.0 (for third-party apps serving many users)

`POST /api/v1/apps` to register, then `/oauth/authorize` + `/oauth/token`. Overkill for personal use — prefer Option A.

## Auth header (every request)

```
Authorization: Bearer <MASTODON_ACCESS_TOKEN>
```

## Verify the token

```
GET $MASTODON_INSTANCE_URL/api/v1/accounts/verify_credentials
```

Returns the authenticated account if the token is valid. Use this as a smoke test.

## Common operations

**Post a status (toot)**
```
POST $MASTODON_INSTANCE_URL/api/v1/statuses
Content-Type: application/json
Body: {"status": "hello fedi", "visibility": "public"}
```

`visibility`: `public` | `unlisted` | `private` (followers only) | `direct` (mentioned only).

**Reply to a status**
```json
{"status": "@handle@instance reply", "in_reply_to_id": "<status_id>"}
```

**Content warning**
```json
{"status": "spicy take", "spoiler_text": "CW: politics", "sensitive": true}
```

**Delete a status**
```
DELETE $MASTODON_INSTANCE_URL/api/v1/statuses/<id>
```

**Home timeline**
```
GET $MASTODON_INSTANCE_URL/api/v1/timelines/home?limit=20
```

**Public federated timeline (without login — does not need auth)**
```
GET $MASTODON_INSTANCE_URL/api/v1/timelines/public?limit=20
```

**Search**
```
GET $MASTODON_INSTANCE_URL/api/v2/search?q=<query>&type=statuses&limit=10
```
`type`: `accounts` | `hashtags` | `statuses`.

**Follow / unfollow**
```
POST $MASTODON_INSTANCE_URL/api/v1/accounts/<id>/follow
POST $MASTODON_INSTANCE_URL/api/v1/accounts/<id>/unfollow
```

## Visibility rules

- `public` and `unlisted` federate across the fediverse.
- `private` reaches only the author's followers.
- `direct` reaches only people `@`-mentioned in the text.

Respect the user's default preference — read it from `verify_credentials` as `source.privacy`.

## Rules

- **Confirm every post.** Toots are public by default and federate permanently. Show the exact text + visibility before sending.
- Character limits vary per instance (default 500, some allow more). `verify_credentials` returns `configuration.statuses.max_characters` — check before truncating.
- Rate limits: 300 requests per 5 minutes per user per instance. Generous for personal use.
- Content warnings (`spoiler_text`) are good etiquette for sensitive topics; suggest them when the content warrants.

## Failure modes

- `401 Unauthorized` → token revoked or wrong scope (e.g. tried to post with a read-only token).
- `422 Unprocessable Entity` → `status` exceeds instance character limit, or `visibility` is an unknown value.
- `404` on another instance's status → Mastodon's federation means non-home statuses are mirrored on demand. Try searching for the URL first to "resolve" it into your instance.
- Slow response → some instances are under-resourced. Not the user's problem to fix; honour the instance's rate limits.
