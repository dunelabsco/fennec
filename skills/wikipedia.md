---
name: wikipedia
description: Query Wikipedia via the MediaWiki Action API for article content, summaries, and search. Use when the user wants encyclopedic background on a topic.
always: false
---

# wikipedia

Wikipedia (and every Wikimedia wiki) exposes the MediaWiki Action API. Public, no key. Use `web_fetch` or `http_request`.

## Base URL

```
https://<lang>.wikipedia.org/w/api.php
```

`<lang>` = `en`, `de`, `fr`, `es`, `ja`, etc. Use the user's language context; default `en`.

## Common operations

**Quick title search**
```
GET /w/api.php?action=opensearch&search=<query>&limit=10&format=json
```
Returns `[query, [titles], [descriptions], [urls]]` as a JSON array.

**Full-text search with snippets**
```
GET /w/api.php?action=query&list=search&srsearch=<query>&srlimit=10&format=json
```

**Article introduction (plain text)**
```
GET /w/api.php?action=query&prop=extracts&exintro&explaintext&titles=<title>&format=json
```
`exintro` + `explaintext` gives you the lead section. Drop `exintro` for the full article (can be very long).

**Article with links, categories, external references**
```
GET /w/api.php?action=query&prop=extracts|categories|links|extlinks&titles=<title>&format=json
```

**REST summary endpoint (faster, cleaner)**
```
GET https://<lang>.wikipedia.org/api/rest_v1/page/summary/<title>
```
Returns `{title, extract, extract_html, description, thumbnail, content_urls}`. Prefer this for short "what is X?" answers.

## Tips

- Titles are case-sensitive for the first letter of each word; URL-encode spaces (`+` or `%20`).
- MediaWiki treats underscores and spaces interchangeably in titles.
- Always set a descriptive `User-Agent`, e.g. `fennec/0.1 (contact@example.com)`. Wikimedia requests this and may throttle generic UAs.
- Rate limits are polite — no hard cap for normal use, but don't hammer.

## Failure modes

- Missing page → `query.pages` has `pageid: -1` with a `missing` key. Tell the user; don't guess.
- Disambiguation page → `extract` reads "X may refer to..." followed by a bulleted list. Detect and ask the user which sense they want.
- Non-English queries in English Wikipedia give weak results — switch the `<lang>` subdomain to match the topic's language.
- Very long extracts — pass `exchars=<N>` or `exsentences=<N>` to truncate.
