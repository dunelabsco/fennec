---
name: go-modules
description: Query the Go module proxy for package versions, metadata, and go.mod files. Use when the user wants to check a Go module's latest version, list releases, or inspect its dependencies. No key required.
always: false
---

# go-modules

Go's module ecosystem is fronted by `proxy.golang.org`, a public immutable cache of every released Go module. Simple HTTP API, no key, no auth.

## Base URL

```
https://proxy.golang.org/
```

Mirrors exist (goproxy.io, goproxy.cn) but proxy.golang.org is the official default and matches what `go mod` uses.

## Module path encoding

**Important quirk:** module paths are case-folded. Uppercase letters in the module path are encoded as `!<lowercase>`. So:

| Real path | Encoded for the proxy |
|---|---|
| `github.com/gin-gonic/gin` | `github.com/gin-gonic/gin` (no change) |
| `github.com/GoRoute/foo` | `github.com/!go!route/foo` |
| `github.com/BurntSushi/toml` | `github.com/!burnt!sushi/toml` |

Forget this and you get `404 Not Found` (different from `410 Gone`, which is reserved for retracted versions — see Failure modes below).

## Common operations

**Latest version + timestamp**
```
GET /<module>/@latest
```
Returns:
```json
{"Version": "v1.9.1", "Time": "2026-03-15T10:42:11Z"}
```

**List all versions**
```
GET /<module>/@v/list
```
Returns a newline-separated list (plain text):
```
v1.0.0
v1.0.1
v1.1.0
...
```

**Specific version metadata**
```
GET /<module>/@v/<version>.info
```
Returns `{"Version": "...", "Time": "..."}`.

**go.mod for a version** (dependencies!)
```
GET /<module>/@v/<version>.mod
```
Returns the raw go.mod file text. Parse for `require` blocks to see the module's dependencies.

**Source zip** (rarely needed from skills — large payload)
```
GET /<module>/@v/<version>.zip
```

## Examples

```
GET https://proxy.golang.org/github.com/gin-gonic/gin/@latest
GET https://proxy.golang.org/github.com/gin-gonic/gin/@v/list
GET https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.9.1.mod
```

## Version format

Semver with a leading `v`: `v1.2.3`, `v1.2.3-beta.1`. Pseudo-versions for untagged commits: `v0.0.0-20260410123456-abcdef012345`. Never trust the proxy's ordering; sort versions yourself by semver.

## Tips

- The proxy is **immutable and append-only** — once a version is published, its `.mod`, `.info`, and `.zip` never change. Great for caching.
- `/@latest` is the only endpoint that moves — it reflects the current latest version.
- To find the repository / docs URL for a module, parse the import path: `github.com/<owner>/<repo>` for GitHub, `gitlab.com/<owner>/<repo>` for GitLab, etc. `pkg.go.dev/<module>` is the canonical docs page.
- For "is there a newer version?" checks, compare the user's locked version against `/@latest` — no more than one request per module per cache period.

## Alternative: pkg.go.dev

For richer metadata (README, open-source license, import count, vulnerability scan), the user-facing site is `https://pkg.go.dev/<module>`. HTML-only; not a JSON API. Scraping is unreliable — the proxy above is the canonical programmatic interface.

## Failure modes

- `404 Not Found` on a real module → case-encoding wrong. Check that uppercase letters are `!`-prefixed.
- `410 Gone` → version was retracted by the author. Still exists for fetch; won't show in `/@latest`.
- `proxy.golang.org returned error: unknown revision` → the version tag doesn't exist in the upstream VCS. Typo or unpublished tag.
- Slow response on first request → the proxy fetches from the origin on cache miss; subsequent requests are fast.
