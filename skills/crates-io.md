---
name: crates-io
description: Query crates.io for Rust crate metadata, versions, and dependencies. Use when the user asks about a Rust library.
always: false
---

# crates-io

crates.io is Rust's package registry with a JSON API. No key, but you MUST set a descriptive `User-Agent` header or the server returns 403.

## Required header

```
User-Agent: fennec/0.1 (<your-email-or-url>)
```

crates.io's policy rejects requests with empty, default, or generic user-agents to discourage misbehaving clients.

## Endpoints

**Crate metadata**
```
GET https://crates.io/api/v1/crates/<name>
```
Returns `{crate, versions, keywords, categories}`. `crate.max_version` is the latest non-yanked release; `versions[]` lists all releases newest-first.

**Specific version**
```
GET https://crates.io/api/v1/crates/<name>/<version>
```

**Dependencies of a specific version**
```
GET https://crates.io/api/v1/crates/<name>/<version>/dependencies
```

**Search**
```
GET https://crates.io/api/v1/crates?q=<query>&per_page=20&sort=relevance
```

Sort options: `relevance`, `downloads`, `recent-downloads`, `recent-updates`, `new`.

## Key fields

- `crate.name`, `crate.description`, `crate.repository`, `crate.homepage`, `crate.documentation`.
- `crate.downloads` — cumulative; `crate.recent_downloads` — last 90 days. Both are popularity signals.
- `crate.max_version` — latest non-yanked version (what `cargo add` picks).
- `versions[].num` — version string.
- `versions[].yanked` — boolean. If true, the maintainer pulled it; treat as "don't use".
- `versions[].features` — feature flags the crate supports.
- `versions[].rust_version` — MSRV (minimum supported Rust version), if declared.

## Tips

- crate names are case-insensitive on lookup but punctuation (hyphens vs underscores) is preserved: `tokio-stream` and `tokio_stream` are different crates.
- `rustdoc`: docs for a specific version are at `https://docs.rs/<name>/<version>/` — link the user there for API reference.
- For deeper popularity / ecosystem context, `lib.rs` (third-party, scrapes crates.io) has useful rankings, but not an API.

## Failure modes

- `403 Forbidden` with empty body → User-Agent is missing or too generic. Set a real one.
- `404 Not Found` → crate name wrong.
- All versions yanked → the crate is effectively dead. Warn the user.
- Large `categories[]` / `keywords[]` list → keywords can be noisy; use the crate description for the "what is this" answer.
