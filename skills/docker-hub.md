---
name: docker-hub
description: Query Docker Hub for image tags, metadata, and search results. Use when the user asks about a container image's available versions, architectures, or a specific image's provenance. Public reads need no auth; private repos need DOCKERHUB_TOKEN.
always: false
---

# docker-hub

Docker Hub's v2 API exposes image metadata, tag lists, and search over the registry. Public images are readable without authentication. Write operations and private-repo reads need a token.

## Base URL

```
https://hub.docker.com/v2/
```

## Optional auth

For anonymous reads of public images — no header needed. Aggressive rate limits apply (~100 pulls per 6 hours for anonymous IPs on the registry side).

For authenticated reads (private repos, higher limits):

1. Sign in at https://hub.docker.com → **Account Settings → Personal access tokens → Generate new token**.
2. Save: `export DOCKERHUB_TOKEN=...`.
3. Mint a JWT:
   ```
   POST https://hub.docker.com/v2/users/login/
   Body: {"username": "<user>", "password": "<DOCKERHUB_TOKEN>"}
   Response: {"token": "<JWT>"}
   ```
4. Use on subsequent requests:
   ```
   Authorization: JWT <JWT>
   ```

(Docker's authenticated header uses `JWT <token>`, not `Bearer`. An uncommon scheme.)

## List tags for an image

**Official images** (`library` namespace):
```
GET https://hub.docker.com/v2/repositories/library/alpine/tags/?page_size=50
GET https://hub.docker.com/v2/repositories/library/node/tags/?page_size=50
GET https://hub.docker.com/v2/repositories/library/python/tags/?page_size=50
```

**User / org images** (include namespace explicitly):
```
GET https://hub.docker.com/v2/repositories/<user>/<repo>/tags/?page_size=50
```

Response shape:
```json
{
  "count": 214,
  "next": "https://hub.docker.com/v2/repositories/library/alpine/tags/?page=2&page_size=50",
  "previous": null,
  "results": [
    {
      "name": "3.20",
      "last_updated": "2026-04-10T...",
      "digest": "sha256:...",
      "images": [
        {"architecture": "amd64", "os": "linux", "size": 3123456, "digest": "sha256:..."},
        {"architecture": "arm64", "os": "linux", "size": 3001234, "digest": "sha256:..."}
      ],
      "tag_status": "active"
    }
  ]
}
```

Pagination via `next` URL. Page sizes up to 100.

Useful filters via query params:
- `name=<substring>` — filter tag names containing a substring.
- `ordering=last_updated` or `-last_updated` — sort order.

## Get specific tag details

```
GET https://hub.docker.com/v2/repositories/library/alpine/tags/3.20/
```

Returns one tag's full manifest list.

## Get repository metadata

```
GET https://hub.docker.com/v2/repositories/library/alpine/
```

Returns description, pull count, star count, last push time, maintainer info.

## Search repositories

```
GET https://hub.docker.com/v2/search/repositories/?query=<term>&page_size=10
```

Response: `results[]` with `repo_name`, `short_description`, `star_count`, `pull_count`.

## What's in `/library/`

Docker Hub official images live under the `library/` namespace — those are the curated, multi-platform, frequently-rebuilt images from the Docker maintainer team. For anything non-official, the namespace is the user or org handle.

## Rules

- Anonymous API reads are fine for occasional lookups. For anything automated or high-volume, use a token — unauthenticated rate limits are sharp.
- Pull counts and star counts are popularity signals but not quality signals. Prefer official (`library/`) images or Verified Publisher images when a choice exists.
- Don't scrape the whole tag list of big images (`library/ubuntu` has thousands). Use `name=` filtering or sort by `-last_updated` and limit results.
- Image tags are **mutable** on Docker Hub — the same tag name can point to different content over time. For reproducible pulls, use the `digest` (`sha256:...`) instead of the tag.

## Failure modes

- `401 Unauthorized` on a private repo → token missing, wrong user, or the JWT expired (JWTs last ~5 hours).
- `404 Not Found` → repo doesn't exist or is private without auth. Double-check the namespace / repo name.
- `429 Too Many Requests` → rate-limited. Add auth, or space requests.
- Response `results: []` on search → term matched nothing. Docker Hub search is not full-text on README; mostly searches name + short description.
- Image `images[]` empty → the manifest may be a list-only index for other architectures; fetch the specific digest to see.

## Related: Docker Registry API (pulling images)

For actual image pulls, Docker uses a separate Registry API at `https://registry-1.docker.io/v2/` with its own token dance (`auth.docker.io/token`). Out of scope here — the registry API is what `docker pull` uses under the hood. This skill covers the Hub metadata API, not the registry itself.
