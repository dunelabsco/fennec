---
name: npm-registry
description: Query the npm registry for package metadata, versions, dependencies, and search. Use when the user asks about a Node / JavaScript package.
always: false
---

# npm-registry

npm's public registry has a JSON HTTP API. No key. Use `http_request` or `web_fetch`.

## Endpoints

**Full package metadata**
```
GET https://registry.npmjs.org/<package>
```
Returns every version ever published with dependencies, tarball URLs, maintainers, descriptions, dist-tags, etc. Can be large — hundreds of KB for popular packages.

**Latest version only (lean)**
```
GET https://registry.npmjs.org/<package>/latest
```
Returns just the current version's metadata: `name`, `version`, `description`, `dependencies`, `devDependencies`, `main`, `types`, `license`, `repository`. Prefer this for quick lookups.

**Specific version**
```
GET https://registry.npmjs.org/<package>/<version>
```

**Search**
```
GET https://registry.npmjs.org/-/v1/search?text=<query>&size=20
```
Response: `objects[].package.{name,version,description,keywords}` plus popularity / quality / maintenance scores.

## Scoped packages

Scoped names like `@types/node` need URL-encoding: `@types%2Fnode`.

```
GET https://registry.npmjs.org/@types%2Fnode/latest
```

## Fields worth knowing

- `version` — semver for this release.
- `dependencies` / `devDependencies` / `peerDependencies` / `optionalDependencies` — dict of `name: version-spec`.
- `engines.node` — minimum Node version.
- `main` / `exports` / `types` — entry points.
- `repository.url` — source URL (usually a GitHub URL).
- `deprecated` (on the version) — if present, the package is deprecated; show the message to the user.
- `time.modified` (top-level metadata) — last publish time; use it to judge activity.

## Tips

- `dist-tags.latest` is what `npm install <name>` resolves to by default.
- For security scanning, point the user at `npm audit` locally — the registry API doesn't surface advisories directly.
- Private / org packages are not accessible via the public API.

## Failure modes

- `404 Not Found` → package never existed, was unpublished, or the name is scoped and not URL-encoded.
- Unpublished packages leave a tombstone response with `{"error": "Not found"}`.
- Huge response on `/<package>` (no `/latest`) → switch to `/<package>/latest`.
