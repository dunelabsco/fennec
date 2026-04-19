---
name: notion
description: Read and write Notion pages, databases, and blocks via the Notion API. Use when the user wants to fetch notes, query a database, or append content to a page. Requires the user's integration token in the NOTION_API_KEY env var.
always: false
---

# notion

Notion exposes a REST API for pages, databases, and blocks. Call endpoints under `https://api.notion.com/v1/` via `http_request`.

## First-time setup (user does this once)

1. Visit https://www.notion.so/my-integrations.
2. Click **New integration** — give it a name (e.g. "Fennec"), pick a workspace, choose **Internal**.
3. Copy the **Internal Integration Secret**. This is the API key.
4. Save it: `export NOTION_API_KEY=secret_...` in shell rc or the agent's config.
5. **Share each page or database** the integration should touch: open the page → `...` menu → **Add connections** → select the integration. The API can only see content explicitly shared.

If `NOTION_API_KEY` is missing at runtime, ask the user to complete setup. Do not fake results.

## Required headers (every request)

```
Authorization: Bearer <NOTION_API_KEY>
Notion-Version: 2025-09-03
Content-Type: application/json
```

`Notion-Version` is mandatory. Pin it to a known version so Notion can't change behaviour under you.

## Common operations

**Search for pages / databases the integration can see**
```
POST https://api.notion.com/v1/search
Body: {"query": "project plan", "filter": {"value": "page", "property": "object"}}
```

**Get page metadata**
```
GET https://api.notion.com/v1/pages/<page_id>
```

**Read page content (blocks)**
```
GET https://api.notion.com/v1/blocks/<page_id>/children?page_size=100
```

**Append to a page**
```
PATCH https://api.notion.com/v1/blocks/<page_id>/children
Body:
{
  "children": [
    {
      "object": "block",
      "type": "paragraph",
      "paragraph": {
        "rich_text": [{"type": "text", "text": {"content": "..."}}]
      }
    }
  ]
}
```

**Query a database**
```
POST https://api.notion.com/v1/databases/<db_id>/query
Body: {"filter": {"property": "Status", "select": {"equals": "Open"}}}
```

**Create a page in a database**
```
POST https://api.notion.com/v1/pages
Body:
{
  "parent": {"database_id": "<db_id>"},
  "properties": {
    "Name": {"title": [{"text": {"content": "New item"}}]}
  }
}
```

## IDs

Notion IDs are UUIDs. The dashed form (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`) and the undashed form (32 hex chars) are both accepted. You can extract an ID from a page URL: the last 32 hex chars of the path.

## Failure modes

- `401 Unauthorized` → `NOTION_API_KEY` is wrong or missing. Ask the user to verify.
- `404 Not Found` on a page you expect to exist → the integration isn't shared with that page. Walk the user through sharing it.
- `400 validation_error` → request body shape is wrong. Double-check the required `properties` for the target page/database; the error message names the offending field.
- `429 rate_limited` → back off (typically 3 requests/second average).

## Rules

- Never echo the integration token into chat or logs.
- When updating a page, read the current state first, modify, then write. Blind overwrites corrupt structure.
- Respect workspace etiquette — don't bulk-create pages without asking.
