---
name: stackexchange
description: Query Stack Overflow and other Stack Exchange sites (Super User, Ask Ubuntu, etc.) via the public API. Use when the user needs a specific programming or sysadmin answer.
always: false
---

# stackexchange

The Stack Exchange API v2.3 is read-only without a key and covers every Stack Exchange site (Stack Overflow, Super User, Ask Ubuntu, Server Fault, etc.).

## Base URL

```
https://api.stackexchange.com/2.3/
```

## Required parameter: `site`

Every request must specify which SE site to query:

| Site | `site` value |
|---|---|
| Stack Overflow | `stackoverflow` |
| Super User | `superuser` |
| Server Fault | `serverfault` |
| Ask Ubuntu | `askubuntu` |
| Unix & Linux | `unix` |
| Software Engineering | `softwareengineering` |

## Common operations

**Search questions by tag + keyword**
```
GET /2.3/search/advanced?q=<query>&tagged=<tag>&site=stackoverflow&order=desc&sort=votes&pagesize=10
```

**Get a specific question with its answers**
```
GET /2.3/questions/<id>?site=stackoverflow&filter=withbody
```
`filter=withbody` includes the question body. Add `&include=answers` to embed answers.

**Get answers for a question**
```
GET /2.3/questions/<id>/answers?site=stackoverflow&filter=withbody&sort=votes
```

**Search by exact tags only**
```
GET /2.3/questions?tagged=rust;tokio&site=stackoverflow&sort=votes&order=desc&pagesize=10
```

Multiple tags joined by `;` = AND.

## Response shape

Responses are gzipped JSON with `items[]` and a `has_more` flag. Common fields on a question:
`question_id`, `title`, `link`, `score`, `answer_count`, `is_answered`, `accepted_answer_id`, `tags`, `body` (with `filter=withbody`).

## Rate limits

- Unauthenticated: 300 requests / day / IP.
- With a free API key: 10,000 / day.
- Throttle violations return `backoff: <seconds>` in the response — honour it.

## Tips

- Always `sort=votes` + `order=desc` for "the accepted wisdom on X".
- `accepted_answer_id` tells you which answer OP marked correct; fetch that one, not just the top-voted.
- Responses arrive gzipped — `http_request` handles decoding transparently.
- Use `site=stackoverflow` for programming; pick the right sibling site for ops / Linux / Mac questions.

## Failure modes

- `400` with `error_id` and `error_name` → check the JSON, usually a bad parameter.
- Empty `items[]` → no matches. Loosen the query.
- `backoff` field present → throttled; wait the specified seconds before next call.
