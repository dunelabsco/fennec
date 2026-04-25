---
name: pubmed
description: Search biomedical literature via NCBI's PubMed (E-utilities API). Use when the user wants medical / life-science papers, abstracts, or MeSH-filtered results.
always: false
---

# pubmed

PubMed covers ~36M citations in medicine, nursing, dentistry, and the life sciences. NCBI's E-utilities REST API gives programmatic access. Free, no key strictly required; an API key raises rate limits.

## Base URL

```
https://eutils.ncbi.nlm.nih.gov/entrez/eutils/
```

## Required parameters on every request

NCBI asks every caller to identify itself:
- `tool=<your-app-name>` — e.g. `fennec`.
- `email=<your-email>` — used only to contact you if something breaks.

Without these, NCBI will throttle aggressively and may eventually block your IP entirely during load events. Treat them as required, not optional. NCBI also rate-limits unauthenticated callers to ~3 req/s — for higher rates request an [API key](https://ncbiinsights.ncbi.nlm.nih.gov/2017/11/02/new-api-keys-for-the-e-utilities/) and pass it via `&api_key=<KEY>`.

## Three endpoints covering most needs

### 1. `esearch.fcgi` — find PMIDs matching a query

```
GET /entrez/eutils/esearch.fcgi?db=pubmed&term=<query>&retmax=20&retmode=json&tool=fennec&email=<email>
```

`term` accepts PubMed's full search grammar:
- Plain keywords: `cancer immunotherapy`
- MeSH terms: `"Immunotherapy"[MeSH]`
- Author: `Smith J[au]`
- Journal: `"N Engl J Med"[ta]`
- Date: `("2023"[dp] : "2026"[dp])`
- Combined with `AND`, `OR`, `NOT`

Response: `{esearchresult: {count, retmax, retstart, idlist: [PMID, ...]}}`.

### 2. `esummary.fcgi` — fast metadata for PMIDs

```
GET /entrez/eutils/esummary.fcgi?db=pubmed&id=<pmid1>,<pmid2>&retmode=json&tool=fennec&email=<email>
```

Returns title, authors, journal, pubdate, volume/issue/pages. Faster than efetch when you only need citation data.

### 3. `efetch.fcgi` — full records (abstract, MeSH, etc.)

```
GET /entrez/eutils/efetch.fcgi?db=pubmed&id=<pmid>&rettype=abstract&retmode=text&tool=fennec&email=<email>
```

- `rettype=abstract` + `retmode=text` → plain text abstract (readable).
- `rettype=abstract` + `retmode=xml` → structured PubMed XML (MeSH terms, publication types, author affiliations).
- `rettype=medline` + `retmode=text` → MEDLINE format (for reference managers).

## History server for large result sets

```
GET /entrez/eutils/esearch.fcgi?db=pubmed&term=<query>&usehistory=y&retmax=0...
# returns WebEnv and query_key
GET /entrez/eutils/efetch.fcgi?db=pubmed&query_key=<k>&WebEnv=<e>&rettype=abstract&retmode=text...
```

Pass `WebEnv` + `query_key` to efetch/esummary to avoid re-sending a list of thousands of PMIDs.

## API key (optional but recommended)

Without a key: 3 requests / second / IP.
With a key: 10 requests / second / IP.

Get one at https://www.ncbi.nlm.nih.gov/account/ → API Key Management. Add to every request:

```
&api_key=<NCBI_API_KEY>
```

Env var: `NCBI_API_KEY` (optional).

## Tips

- Retrieve PMIDs first with esearch, then fetch in batches of 200 via esummary/efetch.
- For "recent papers on X", combine: `...&reldate=30&datetype=edat` (articles indexed in last 30 days).
- PubMed IDs (PMIDs) are short numeric (e.g. `36789012`); do not confuse with DOIs.
- For systematic reviews, also consult the `crossref` skill — PubMed is biomed-only.

## Response formats

| Combination | Output |
|---|---|
| `retmode=json` | JSON (esearch/esummary) |
| `retmode=xml` | Full PubMed XML (efetch) |
| `retmode=text` + `rettype=abstract` | Plain-text abstract |
| `retmode=text` + `rettype=medline` | MEDLINE format for reference managers |

## Failure modes

- `<ERROR>` element in XML response → term syntax error. Read the error; common issue is unbalanced quotes or brackets.
- `Supplied id is not valid` → PMID doesn't exist or is from another database.
- Silent throttling / hangs → you exceeded 3 req/s without a key. Add the key or slow down.
- Empty `idlist` → the term matched nothing. MeSH terms are strict — try free-text first.

## Related

- Crossref: broader (all academic disciplines); use for DOI lookups and non-biomed work.
- Semantic Scholar: citation graph, OA PDF links, cross-domain.
