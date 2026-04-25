---
name: arxiv
description: Search arxiv.org for research papers via its public API. Use when the user asks about a paper, wants recent work on a topic, or needs an abstract.
always: false
---

# arxiv

arxiv.org has a free, no-key HTTP API that returns search results as Atom 1.0 XML. Use `web_fetch` or `http_request` to hit it.

## Endpoint

```
https://export.arxiv.org/api/query?search_query=<query>&start=<offset>&max_results=<n>
```

- `search_query` — required. Field-prefixed query string (see below).
- `start` — optional, default 0. Result offset for pagination.
- `max_results` — optional, default 10. The API accepts large values but prefer 5–20 for readable output.

The response body is Atom XML. Each matching paper is a `<entry>` with `<title>`, `<summary>` (abstract), one or more `<author>`, `<published>`, `<id>` (canonical arxiv URL), and `<link>` elements (the HTML abs page and the PDF).

## Query fields

Prefix each term with the field you want to search:

| Prefix | Field |
|---|---|
| `ti:` | Title |
| `au:` | Author |
| `abs:` | Abstract |
| `cat:` | Category (`cs.LG`, `math.AG`, `physics.hep-ph`, etc.) |
| `co:` | Comment |
| `jr:` | Journal reference |
| `rn:` | Report number |
| `id:` | arxiv id (prefer the dedicated `id_list` parameter instead) |
| `all:` | Any field |

Combine with `AND`, `OR`, `ANDNOT`. Quote multi-word phrases: `ti:"attention is all you need"`.

Example — recent ML transformers:

```
https://export.arxiv.org/api/query?search_query=cat:cs.LG+AND+abs:transformer&max_results=10&sortBy=submittedDate&sortOrder=descending
```

URL-encode spaces as `+` or `%20`.

## Rate limits

arxiv asks for **no more than one request every 3 seconds** per IP. Respect it — don't fire parallel queries against the API. If you need multiple searches, space them.

## Parsing the response

You get XML text. Pull fields by matching the tags — the structure is stable. Per `<entry>`:

- Title: between `<title>` and `</title>` (use the one inside `<entry>`, not the feed-level one).
- Abstract: between `<summary>` and `</summary>`.
- Authors: the `<name>` inside each `<author>`.
- Published: the `<published>` element, ISO 8601.
- PDF URL: the `<link>` with `title="pdf"` has the `href` to the PDF.
- abs URL: `<id>` is the canonical page.

No need for a full XML parser — treat it as text and pattern-match.

## Presenting results

- For "find me papers on X": 3–5 entries, one-liner each: `"Author et al., YYYY — Title" → url`. Let the user pick one to dig into.
- For "summarise paper ID": fetch the PDF with `pdf_read` and then condense using the `summarize` pattern. Do not paraphrase the abstract — abstracts are already the authors' own summary. Quote a short line if useful.
- Always cite the arxiv URL. That is the canonical reference.

## Failure modes

- `HTTP 429` or rate-limited response → you sent too fast. Back off several seconds, then retry once.
- Empty feed with no `<entry>` elements → the query matched nothing. Loosen terms or remove a constraint. Do not invent papers.
- Malformed XML / truncated response → retry once; if still broken, report the upstream issue rather than guessing.
- Unicode / non-ASCII titles missing characters → that is arxiv's own encoding; nothing to fix client-side.
