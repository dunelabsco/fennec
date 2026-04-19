---
name: trello
description: Manage Trello boards, lists, and cards via the REST API. Use when the user wants to add a card, move it across lists, list what's on a board, or inspect/update comments. Requires TRELLO_API_KEY and TRELLO_API_TOKEN env vars.
always: false
---

# trello

Trello's REST API auth is old-school: an API key (from a Power-Up) plus a user-authorized token, both passed as query parameters on every request.

## First-time setup

Trello's manual API-key page was retired; today you register a **Power-Up** to get a key:

1. Go to https://trello.com/power-ups/admin → **New** → fill in a name (e.g. "Fennec"), pick a workspace.
2. In the Power-Up's settings, go to **API Key** → **Generate a new API Key**. Copy it.
3. On that same page, click **manually generate a Token** (or use the URL below). This opens an authorization page; approve.
   ```
   https://trello.com/1/authorize?expiration=never&name=Fennec&scope=read,write&response_type=token&key=<YOUR_API_KEY>
   ```
   After approval, Trello shows a token string — copy it.
4. Save both:
   ```
   export TRELLO_API_KEY="..."
   export TRELLO_API_TOKEN="..."
   ```

`expiration=never` gives a long-lived token. Use shorter expirations (`1hour`, `1day`, `30days`) for more cautious setups.

## Auth on every request

Append as query params — Trello uses query-string auth, **not** a header:

```
?key=<TRELLO_API_KEY>&token=<TRELLO_API_TOKEN>
```

## Base URL

```
https://api.trello.com/1/
```

## Verify the token

```
GET /1/members/me?key=<KEY>&token=<TOKEN>
```

Returns the authenticated member. Quick smoke test.

## Boards

**List my boards**
```
GET /1/members/me/boards?fields=name,url,closed&filter=open&key=<KEY>&token=<TOKEN>
```

**Get one board**
```
GET /1/boards/<board_id>?fields=name,desc,url&key=<KEY>&token=<TOKEN>
```

Trello IDs are 24-char hex strings. Board URLs look like `https://trello.com/b/<short_id>/<slug>` — the short ID is not the same as the 24-char ID; use the API to resolve if needed.

## Lists (columns)

**Lists on a board**
```
GET /1/boards/<board_id>/lists?cards=open&fields=name,closed,pos&key=<KEY>&token=<TOKEN>
```

`cards=open` embeds non-archived cards per list — one round trip for a full board snapshot.

## Cards

**Cards on a list**
```
GET /1/lists/<list_id>/cards?fields=name,desc,due,labels,idMembers&key=<KEY>&token=<TOKEN>
```

**Get one card**
```
GET /1/cards/<card_id>?fields=name,desc,due,labels,idMembers,closed&key=<KEY>&token=<TOKEN>
```

**Create a card**
```
POST /1/cards?key=<KEY>&token=<TOKEN>
Body (form or JSON):
  idList=<list_id>
  name=<card title>
  desc=<card body>
  due=2026-04-24T15:00:00.000Z          # optional; ISO 8601
  pos=top                                # top | bottom | number
```

**Update a card** (rename, change description, set due, move)
```
PUT /1/cards/<card_id>?key=<KEY>&token=<TOKEN>
Body: {"name": "new name", "desc": "...", "idList": "<dest_list_id>"}
```

**Archive (close) a card**
```
PUT /1/cards/<card_id>?key=<KEY>&token=<TOKEN>
Body: {"closed": true}
```

**Delete** (irreversible)
```
DELETE /1/cards/<card_id>?key=<KEY>&token=<TOKEN>
```

**Add a comment**
```
POST /1/cards/<card_id>/actions/comments?key=<KEY>&token=<TOKEN>
Body: {"text": "Comment body"}
```

## Members, labels, checklists

```
GET  /1/boards/<board_id>/members        # people on the board
POST /1/cards/<card_id>/idMembers         # body: {"value": "<member_id>"}
GET  /1/boards/<board_id>/labels
POST /1/cards/<card_id>/idLabels          # body: {"value": "<label_id>"}
POST /1/cards/<card_id>/checklists        # body: {"name": "Todo"}
POST /1/checklists/<checklist_id>/checkItems   # body: {"name": "task", "checked": false}
```

## Rules

- **Don't delete boards, lists, or cards silently.** Destructive actions are irreversible in Trello. Confirm with the user each time.
- Token leakage is a full-account exposure. Don't log it; don't embed it in gist / paste output; never commit.
- `pos=top` / `pos=bottom` / a number — Trello positions cards by float ("lexicographic position"). To move a card between two others, set `pos` to the mid-value of the neighbours.
- Webhooks are a thing but out of scope here — for real-time sync, look at Trello's webhook endpoints.
- Rate limits: 300 requests / 10 seconds / API key, 100 / 10s / token. Generous for personal use.

## Failure modes

- `401 Unauthorized` → key or token wrong, or token expired (if you set an expiration).
- `400 invalid id` → 24-char hex ID wrong, or you passed a short board URL ID instead of the full one.
- `404 Not Found` → the board/list/card exists but your token's account doesn't have access.
- `429 Too Many Requests` → slow down; responses include a `Retry-After` header.
- Card body with special characters renders wrong → Trello supports markdown in `desc`; if you're generating content, double-check line breaks (blank line between paragraphs).
