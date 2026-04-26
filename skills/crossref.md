---
name: crossref
description: Look up DOI metadata and search scholarly works via the Crossref REST API. Use when the user has a DOI, wants to cite a paper correctly, or needs to find publications by title / author.
always: false
---

# crossref

Crossref is the DOI registrar for most academic publishing. Their REST API exposes ~180M scholarly items (journal articles, conference papers, books, datasets) with structured metadata. Free, no key, but you should identify yourself.

## Base URL

```
https://api.crossref.org/
```

## Polite pool — identify yourself

Crossref runs two rate tiers. Add your email via `mailto=` (or `User-Agent: <app>/<version> (mailto:you@example.com)`) and you land in the "polite pool" — faster, more reliable, unlikely to get throttled during heavy periods.

```
GET https://api.crossref.org/works?query=...&mailto=you@example.com
```

or as a header:
```
User-Agent: fennec/0.1 (mailto:you@example.com)
```

Without an identifier you go into the "anonymous" pool — works, but slower and more prone to throttling.

## Common operations

**Lookup by DOI** (direct, fastest)
```
GET https://api.crossref.org/works/10.1038/nature12373
```

Response: `{status, message-type: "work", message: {DOI, title, author[], published, container-title, ...}}`.

**Search by title**
```
GET https://api.crossref.org/works?query.title=attention+is+all+you+need&rows=5&select=DOI,title,author,published
```

`select=` narrows which fields come back — large responses otherwise.

**Search by author**
```
GET https://api.crossref.org/works?query.author=Hinton&rows=10
```

**Combined search with filters**
```
GET https://api.crossref.org/works?query.title=transformer&filter=from-pub-date:2023,type:journal-article&rows=10
```

Useful `filter` values (comma-separated):
- `from-pub-date:YYYY-MM-DD`, `until-pub-date:YYYY-MM-DD`
- `type:journal-article` / `type:book-chapter` / `type:proceedings-article`
- `has-full-text:true`
- `is-update:false` (skip errata/corrections)
- `member:<crossref-member-id>` (filter by publisher)

**Sort**
```
...&sort=published&order=desc
```

Valid `sort`: `relevance`, `published`, `indexed`, `updated`, `references-count`, `is-referenced-by-count`.

## Response fields worth knowing

- `DOI` — canonical identifier.
- `title` — array (usually one element).
- `author` — array of `{given, family, ORCID?, affiliation[]}`.
- `published` / `published-online` / `published-print` — structured date `{date-parts: [[YYYY, MM, DD]]}`.
- `container-title` — journal / book name.
- `volume`, `issue`, `page`.
- `abstract` — may be HTML-wrapped (JATS XML sometimes).
- `reference[]` — what the work cites (when the publisher deposits it).
- `is-referenced-by-count` — citation count (known to Crossref).
- `URL` — publisher landing page.

## Tips

- DOIs are case-insensitive when looking up; use the lowercase form in URLs.
- The polite-pool identifier is worth the one line of code. Don't skip it for production use.
- `abstract` field isn't always present — many publishers don't deposit abstracts.
- For full-text / PDF: `link[]` sometimes contains open-access URLs but Crossref doesn't mirror content. Use `semantic-scholar` skill for OA link discovery.
- For citation formatting (BibTeX, RIS), use DOI content negotiation:
  ```
  curl -L -H "Accept: application/x-bibtex" https://doi.org/10.1038/nature12373
  ```

## Failure modes

- `404 Not Found` on a DOI → typo, or the DOI is registered with a different agency (DataCite, mEDRA, etc.). Try `https://doi.org/<DOI>` in a browser to see.
- `429 Too Many Requests` → hit the anonymous rate limit. Switch to the polite pool via `mailto=`.
- Empty `items[]` on a search that should match → Crossref indexes take days to weeks for new publications. Try a narrower or different query.
- Large payloads → use `select=` to limit fields.
