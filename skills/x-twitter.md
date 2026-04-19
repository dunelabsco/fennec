---
name: x-twitter
description: Post to X (Twitter), search posts, and read timelines via the X API v2. Use when the user wants to publish a tweet, search for tweets, or fetch their own timeline. Requires OAuth 2.0 PKCE setup; access token in X_ACCESS_TOKEN env var.
always: false
---

# x-twitter

X (Twitter) API v2 uses OAuth 2.0 with PKCE for user-authenticated access. Personal use requires a developer account + app setup. The access tokens are short-lived; a refresh token keeps things fresh.

## First-time setup

1. Apply for a developer account at https://developer.x.com — free tier suffices for personal use, but posting requires at least the "Free" tier (~500 posts/month as of late 2026; verify current limits).
2. Create a Project + App in the dev portal.
3. Enable **User authentication settings**:
   - Type of App: **Native App** (public client — no client secret needed with PKCE).
   - Callback URL: `http://localhost:8080/callback` (any localhost works; match what you pass in the auth flow).
   - App permissions: **Read and Write** (required for posting).
4. Copy the **Client ID** from the app's Keys and Tokens tab.
5. Run an OAuth 2.0 PKCE flow once to get a refresh token:
   - Build auth URL:
     ```
     https://x.com/i/oauth2/authorize
       ?response_type=code
       &client_id=<CLIENT_ID>
       &redirect_uri=http://localhost:8080/callback
       &scope=tweet.read%20tweet.write%20users.read%20offline.access
       &state=<random>
       &code_challenge=<sha256(code_verifier) base64url>
       &code_challenge_method=S256
     ```
   - User visits it, approves, redirected to `http://localhost:8080/callback?code=...`.
   - Exchange the code:
     ```
     POST https://api.x.com/2/oauth2/token
     Content-Type: application/x-www-form-urlencoded

     grant_type=authorization_code
     code=<code>
     client_id=<CLIENT_ID>
     redirect_uri=http://localhost:8080/callback
     code_verifier=<code_verifier>
     ```
   - Response: `{"access_token": "...", "refresh_token": "...", "expires_in": 7200}`.
6. Save: `export X_ACCESS_TOKEN=...` (short-lived) + `export X_REFRESH_TOKEN=...` (long-lived) + `export X_CLIENT_ID=...`.

## Refresh a stale token

```
POST https://api.x.com/2/oauth2/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
refresh_token=<X_REFRESH_TOKEN>
client_id=<X_CLIENT_ID>
```

Response carries a new `access_token` (and possibly a rotated refresh token — store it).

## Auth header (every API call)

```
Authorization: Bearer <X_ACCESS_TOKEN>
```

## Common operations

**Post a tweet**
```
POST https://api.x.com/2/tweets
Content-Type: application/json
Body: {"text": "hello from fennec"}
```
Max 280 chars for standard accounts, 25k for verified premium.

**Reply to a tweet**
```json
{"text": "...", "reply": {"in_reply_to_tweet_id": "<id>"}}
```

**Quote tweet**
```json
{"text": "...", "quote_tweet_id": "<id>"}
```

**Delete a tweet**
```
DELETE https://api.x.com/2/tweets/<id>
```

**My user info**
```
GET https://api.x.com/2/users/me
```

**Search recent tweets (last 7 days)**
```
GET https://api.x.com/2/tweets/search/recent?query=from:username%20%23topic&max_results=10
```

## Scope summary

| Scope | What it unlocks |
|---|---|
| `tweet.read` | Read tweets |
| `tweet.write` | Post, delete |
| `users.read` | Look up user info |
| `offline.access` | Get a refresh token (required for long-lived access) |

Always include `offline.access` in the initial auth request; otherwise the refresh flow doesn't work and the user has to re-auth every 2 hours.

## Rules

- **Never post without explicit user confirmation.** Show the full draft and wait for "yes".
- Access tokens last 2 hours. Check for 401 and auto-refresh before retrying.
- Rate limits are aggressive on the free tier — honour `x-rate-limit-remaining` / `x-rate-limit-reset` headers.
- `text` is HTML-unsafe — users see whatever you send. Escape nothing; trust user intent.
- Media uploads are a two-step process (upload via v1.1 media endpoint, then reference `media_ids` in v2 tweet). If the user asks to post an image, confirm and walk the steps.

## Failure modes

- `401 Unauthorized` → token stale; refresh.
- `403 Forbidden` → app permissions are read-only (set "Read and Write" in dev portal).
- `429 Too Many Requests` → respect `x-rate-limit-reset`.
- `400 `unauthorized_client`` during token exchange → `client_id` or `redirect_uri` mismatches what's in the dev portal app config.
- Tweet posts but replies never arrive → recipient has muted/blocked the user or app. Not detectable via API.
