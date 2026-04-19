---
name: pypi
description: Query the PyPI registry for Python package metadata, versions, and dependencies. Use when the user asks about a Python package.
always: false
---

# pypi

PyPI provides a JSON API for package metadata. Search is weaker than npm — PyPI removed their official JSON search endpoint. Use the JSON API for metadata; point the user at the web UI for discovery searches.

## Endpoints

**Package metadata (latest version)**
```
GET https://pypi.org/pypi/<package>/json
```
Returns `{info: {name, version, summary, description, requires_dist, home_page, author, classifiers, ...}, releases: {...}, urls: [...]}`.

**Specific version**
```
GET https://pypi.org/pypi/<package>/<version>/json
```

**All released versions (keys of the `releases` dict in the latest metadata)**

From the `/<package>/json` response, `releases` is keyed by version string. Sort by PEP 440 to get the version list.

## Searching

PyPI's old JSON search returned 410 Gone. Current options:

- **Web UI**: https://pypi.org/search/?q=<query> — HTML, not an API.
- **PyPI Simple Index**: `https://pypi.org/simple/` — a huge HTML list of every package name. Useful for autocomplete-like lookups; too broad for semantic search.
- **pip locally**: `pip index versions <package>` lists known versions. `pip search` is disabled.
- **Third-party search**: `https://pypi.org/search/?q=<query>&format=json` is unofficial / undocumented — do not rely on it.

For a "find me a Python package that does X" query, tell the user PyPI's search is web-only and offer to open the URL.

## Fields

- `info.version` — current release.
- `info.summary` — one-line description.
- `info.description` — long description (often the README); can be very long.
- `info.requires_dist` — list of PEP 508 dependency strings like `"requests>=2.0"`, `"numpy; python_version>='3.8'"`. Includes optional and environment-conditional deps.
- `info.requires_python` — supported Python versions as a specifier.
- `info.classifiers` — `"License :: OSI Approved :: MIT License"`, `"Programming Language :: Python :: 3.11"`, etc. Read these for quick license / Python compatibility signal.
- `info.home_page`, `info.project_urls` — documentation / source / issue tracker links.
- `urls[]` — distribution files (wheels and sdists) for the current version.

## Tips

- Package names are case-insensitive in the URL path, but exact case appears in the response `info.name`.
- Normalisation: PyPI treats `Foo_Bar.Baz` and `foo-bar-baz` as the same package. The canonical form is lowercase with hyphens.
- Deprecation / yank: a yanked release still appears in `releases` but can't be newly installed. PyPI doesn't have a single `deprecated` flag like npm.

## Failure modes

- `404 Not Found` → package doesn't exist. Check the spelling; consider the normalised form.
- Very large `info.description` → strip it before showing the user, or use `info.summary` instead.
- Multiple dependency specifiers with environment markers — parse each line of `requires_dist` separately.
