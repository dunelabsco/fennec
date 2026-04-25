---
name: pastebin
description: Publish text dumps (logs, code snippets, config) to GitHub Gist or pastebin.com for easy sharing via URL. Use when the content is too large to paste into chat or needs a permanent URL.
always: false
---

# pastebin

Two paths. **GitHub Gist** is recommended — permanent URLs, syntax highlighting, revision history. **pastebin.com** is a classic fallback for anonymous one-offs.

## GitHub Gist (recommended)

### Setup

1. Generate a personal access token at https://github.com/settings/tokens:
   - Fine-grained: enable **"Gists: Read and write"**.
   - Classic: enable the `gist` scope.
2. Save it: `export GITHUB_TOKEN=ghp_...` or `export GIST_TOKEN=ghp_...`.

(If the user already has `GITHUB_TOKEN` from other tooling and it has the gist scope, reuse it.)

### Create a gist

```
POST https://api.github.com/gists
Headers:
  Authorization: Bearer <GITHUB_TOKEN>
  Accept: application/vnd.github+json
  X-GitHub-Api-Version: 2022-11-28
Body:
{
  "description": "what this is",
  "public": false,
  "files": {
    "snippet.py": {"content": "print('hello')"}
  }
}
```

Response includes `html_url` (the browser URL) and `id` (for later edits).

### Update a gist
```
PATCH https://api.github.com/gists/<id>
Body: {"files": {"snippet.py": {"content": "<new content>"}}}
```

### Multiple files in one gist
Each key under `files` is a separate file:
```json
"files": {
  "app.py": {"content": "..."},
  "requirements.txt": {"content": "..."}
}
```

GitHub auto-detects language from extensions. Use `.md` for rendered markdown.

## pastebin.com (legacy alternative)

### Setup

1. Create a free account at https://pastebin.com.
2. Get the dev key at https://pastebin.com/doc_api#1. Save as `PASTEBIN_API_KEY`.

### Create a paste

```
POST https://pastebin.com/api/api_post.php
Content-Type: application/x-www-form-urlencoded

api_dev_key=<PASTEBIN_API_KEY>
api_option=paste
api_paste_code=<content>
api_paste_private=1              # 0 public, 1 unlisted, 2 private (requires api_user_key, see below)
api_paste_expire_date=1H         # N|10M|1H|1D|1W|2W|1M|6M|1Y
api_paste_format=python          # optional syntax hint
api_paste_name=<title>           # optional
```

Response body is the paste URL (plain text) or an error starting with `Bad API request`.

### Private (account-bound) pastes

`api_paste_private=2` requires an `api_user_key` tied to your account. Mint it with a one-time login call:

```
POST https://pastebin.com/api/api_login.php
Content-Type: application/x-www-form-urlencoded

api_dev_key=<PASTEBIN_API_KEY>
api_user_name=<account username>
api_user_password=<account password>
```

Response body is the user key (plain text). Save it as `PASTEBIN_USER_KEY`, then add `api_user_key=<PASTEBIN_USER_KEY>` to the create-paste form. The user key doesn't expire on its own.

## Rules

- **Never paste secrets, API keys, credentials, or private user data.** Even "unlisted" gists and pastes are accessible to anyone with the URL; they get indexed by scrapers within minutes.
- Public by default? No — use `public: false` for gists, `api_paste_private=1` for pastebin. Confirm with the user before making public.
- Large pastes (> 1 MB): GitHub's gist limit is 1 MB per file and 100 files per gist; pastebin is 512 KB. Split if needed.
- Tell the user the URL on success and remind them who can see it.

## Failure modes

- Gist `401 Unauthorized` → token expired or wrong scope.
- Gist `403` with rate-limit headers → hit the 5000/hour authenticated limit; back off.
- Pastebin `Bad API request, invalid api_dev_key` → key wrong or account suspended.
- Pastebin `Bad API request, invalid paste option` → `api_paste_format` string isn't in their list; drop it or pick from their accepted formats.
- HTML returned instead of expected response → most often pastebin throttling or a captcha wall; slow down or switch to gist.
