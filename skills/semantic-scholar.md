---
name: semantic-scholar
description: Query Semantic Scholar for academic papers, citations, and authors. Use when the user wants peer-reviewed or academic results richer than arxiv (citation graph, fields of study, open-access links).
always: false
---

# semantic-scholar

Semantic Scholar's Graph API covers ~200M papers across sciences, with citation graphs, author info, and links to open-access PDFs. No key required for basic use.

## Base URL

```
https://api.semanticscholar.org/graph/v1/
```

## Common operations

**Search papers by keyword**
```
GET /graph/v1/paper/search?query=<terms>&limit=10&fields=title,authors,year,abstract,openAccessPdf,citationCount
```

**Bulk search** (faster, better for ranking):
```
GET /graph/v1/paper/search/bulk?query=<terms>&limit=100&sort=relevance
```

**Get one paper by ID**
```
GET /graph/v1/paper/<id>?fields=title,authors,abstract,citations,references,openAccessPdf
```

IDs can be: Semantic Scholar ID (hex), DOI prefix `DOI:10.1234/...`, arXiv prefix `ARXIV:2301.12345`, PMID prefix `PMID:12345`.

**Citations (who cited this paper)**
```
GET /graph/v1/paper/<id>/citations?fields=title,authors,year&limit=20
```

**References (what this paper cites)**
```
GET /graph/v1/paper/<id>/references?fields=title,authors,year&limit=20
```

**Author by ID**
```
GET /graph/v1/author/<id>?fields=name,affiliations,papers.title,papers.year
```

## Fields (pick what you need)

Requesting all fields returns huge payloads. Common picks:
- `title, authors, year` — compact listing.
- `+ abstract` — readable summary.
- `+ openAccessPdf` — direct PDF URL when available.
- `+ citationCount, influentialCitationCount` — popularity signals.
- `+ fieldsOfStudy, s2FieldsOfStudy` — topic tagging.

## API keys

Optional but useful:
- **No key**: shared rate limit across all unauthenticated clients (can be slow at peak times).
- **Free key**: dedicated 1 req/sec. Request one at https://www.semanticscholar.org/product/api.

If you have a key, add header:
```
x-api-key: <SEMANTIC_SCHOLAR_API_KEY>
```

Env var: `SEMANTIC_SCHOLAR_API_KEY` (optional).

## Tips

- `abstract` is often plain text; no need to strip HTML.
- `openAccessPdf.url` is the direct PDF — pair with Fennec's `pdf_read` for full-text reads.
- For recent preprints, cross-reference with the `arxiv` skill.
- Papers can have multiple IDs (DOI + arXiv + PubMed); any works.

## Failure modes

- `429 Too Many Requests` → throttled, slow down. With a key you get a dedicated budget.
- Empty `data[]` → no matches. Broaden terms.
- `openAccessPdf: null` → no open-access version known; the user may need library access.
