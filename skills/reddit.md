---
name: reddit
description: Read and post to Reddit via the official API. Use when the user wants to search subreddits, read threads, post a comment, or submit a post. Requires REDDIT_CLIENT_ID + REDDIT_CLIENT_SECRET + REDDIT_USERNAME + REDDIT_PASSWORD env vars.
always: false
---

# reddit

Reddit's API uses OAuth 2.0. For personal use, register a "script"-type app — the simplest flow, uses the user's own credentials and skips the web redirect dance.

## First-time setup

1. Go to https://www.reddit.com/prefs/apps → **create another app** (or "are you a developer? create an app…" if first-time).
2. Fill in:
   - **App name**: e.g. "Fennec".
   - **App type**: pick **script** (for personal use) — not "web app" or "installed app".
   - **about url / redirect uri**: put `http://localhost:8080` (required field, not actually used for script apps).
3. Create the app. You'll see:
   - **Client ID**: the short string under the app name (~14 chars).
   - **Client secret**: the longer string labeled "secret".
4. Save:
   ```
   export REDDIT_CLIENT_ID="..."
   export REDDIT_CLIENT_SECRET="..."
   export REDDIT_USERNAME="yourusername"
   export REDDIT_PASSWORD="yourpassword"
   export REDDIT_USER_AGENT="fennec/0.1 (by /u/yourusername)"
   ```

## User-Agent is required

Reddit **actively rejects** requests without a descriptive User-Agent. Use a string that identifies your app and contact: `fennec/0.1 (by /u/yourusername)`. Generic UAs get 429'd fast.

## Get an access token

```
POST https://www.reddit.com/api/v1/access_token
Authorization: Basic <base64(CLIENT_ID:CLIENT_SECRET)>
User-Agent: <REDDIT_USER_AGENT>
Content-Type: application/x-www-form-urlencoded

grant_type=password&username=<USERNAME>&password=<PASSWORD>
```

At the shell:
```bash
BASIC=$(printf '%s:%s' "$REDDIT_CLIENT_ID" "$REDDIT_CLIENT_SECRET" | base64)
curl -X POST https://www.reddit.com/api/v1/access_token \
  -H "Authorization: Basic $BASIC" \
  -H "User-Agent: $REDDIT_USER_AGENT" \
  -d "grant_type=password&username=$REDDIT_USERNAME&password=$REDDIT_PASSWORD"
```

Response: `{"access_token": "...", "token_type": "bearer", "expires_in": 3600, "scope": "*"}`.

Token lasts **1 hour** (3600 s) — Reddit's OAuth2 wiki states "All bearer tokens expire after 1 hour." Refresh by re-running the password grant (script-type apps don't get a `refresh_token`). On 401, mint a new one.

## API base URL

**Once you have a token, requests go to `oauth.reddit.com`, NOT `www.reddit.com`.** This is a common footgun.

```
Authorization: Bearer <access_token>
User-Agent: <REDDIT_USER_AGENT>
```

## Common operations

**Current user info (verifies token)**
```
GET https://oauth.reddit.com/api/v1/me
```

**Hot posts in a subreddit**
```
GET https://oauth.reddit.com/r/rust/hot?limit=25
```

Other sorts: `/hot`, `/new`, `/top`, `/rising`, `/controversial`. For `/top`, append `?t=day|week|month|year|all`.

**Get a specific post with comments**
```
GET https://oauth.reddit.com/r/rust/comments/<post_id>
```

Returns an array: `[post_listing, comment_listing]`. Comments are nested trees; the `replies` field contains deeper levels. Comments beyond a certain depth are stubs ("continue this thread") — fetch them with `/api/morechildren`.

**Search**
```
GET https://oauth.reddit.com/search?q=<query>&sort=relevance&t=week&limit=25
GET https://oauth.reddit.com/r/rust/search?q=<query>&restrict_sr=1&limit=25
```

`restrict_sr=1` limits search to a single subreddit.

**Submit a post (link or self-text)**
```
POST https://oauth.reddit.com/api/submit
Content-Type: application/x-www-form-urlencoded

sr=rust
kind=self
title=<title>
text=<markdown body>            # for kind=self
url=<link url>                   # for kind=link
api_type=json                    # always set; returns usable JSON
```

**Post a comment**
```
POST https://oauth.reddit.com/api/comment
Content-Type: application/x-www-form-urlencoded

thing_id=t3_<post_id>            # or t1_<comment_id> to reply to a comment
text=<markdown comment>
api_type=json
```

Thing prefixes:
- `t1_` — comment
- `t3_` — post (submission)
- `t5_` — subreddit
- `t2_` — account

**Vote**
```
POST https://oauth.reddit.com/api/vote
id=t3_<post_id>
dir=1                            # 1 up, -1 down, 0 remove vote
```

Votes are reversible — send `0` to clear or the opposite direction to flip. Re-sending the same direction is a no-op (Reddit doesn't stack votes from the same account).

**Save / unsave for later**
```
POST https://oauth.reddit.com/api/save            # body: id=t3_<post_id>
POST https://oauth.reddit.com/api/unsave          # body: id=t3_<post_id>
```

## Pagination

Reddit paginates with `after` / `before` tokens, not page numbers. Each listing response contains `data.after`. Pass it back on the next request:
```
GET .../hot?limit=25&after=t3_abc123
```

An empty / null `after` means last page.

## Markdown quirks

Reddit uses its own markdown dialect:
- Use two newlines for paragraph breaks (single newline is ignored).
- `>` for blockquotes.
- `* item` or `- item` for lists — must have a blank line before the list.
- `^^` + text + `^^` for superscript.
- `~~text~~` for strikethrough.
- Code blocks: four-space indent OR triple-backticks (newer rendering).

## Rules

- **Confirm before posting / commenting / voting.** These are public actions on a user's account.
- Reddit rate limits are sharp: 60 requests/minute for authenticated requests. Check `X-Ratelimit-*` response headers.
- Many subreddits have spam filters that auto-delete posts from new accounts or accounts with low karma. The API will report success but the post won't appear publicly.
- `oauth.reddit.com` for all API calls post-auth. Hitting `www.reddit.com` with a bearer token returns 200 OK but the response is HTML, not JSON — confusing.
- Store tokens in memory for the session only; re-mint rather than persist. Reddit passwords are especially sensitive.

## Failure modes

- `{"error": 401}` without HTML → bad credentials during token exchange.
- HTML response when you expected JSON → you hit `www.reddit.com` instead of `oauth.reddit.com`. Switch.
- `403 Forbidden` → subreddit is private, or the account is banned/suspended.
- `429 Too Many Requests` → hit the rate limit. Back off per `X-Ratelimit-Reset`.
- Post submits but doesn't appear → auto-moderator filter. Check the subreddit's modlog (if public) or contact mods.
- `SUBREDDIT_NOEXIST` error code → typo or banned subreddit.
