---
name: shortened-url
description: Shorten long URLs using is.gd or TinyURL (no key). Use when the user wants a compact link to share in chat or print, or to produce a click-tracking-free short URL.
always: false
---

# shortened-url

Two no-key options for ad-hoc URL shortening. Both return the shortened URL as plain text.

## is.gd

```
GET https://is.gd/create.php?format=simple&url=<ORIGINAL_URL>
```

URL-encode the `url` parameter. Response body is the shortened URL (e.g. `https://is.gd/abcd12`). No body wrapper, no JSON — just the string.

With a custom slug:
```
GET https://is.gd/create.php?format=simple&url=<ORIGINAL_URL>&shorturl=<custom-slug>
```

Returns the short URL or an error message if the slug is taken.

## TinyURL

```
GET https://tinyurl.com/api-create.php?url=<ORIGINAL_URL>
```

Same idea — URL-encode, response is plain text with the short URL.

## When to prefer which

- **is.gd** — shorter slugs (4–5 chars), supports custom aliases, minimal redirect overhead.
- **TinyURL** — longer-established, more likely to resolve on old email clients or printed materials.

## Rules

- **Never** shorten a URL that contains credentials, tokens, or session IDs. The target URL is visible to the shortener and possibly logged.
- Don't shorten internal URLs (localhost, intranet) — they'll 404 for anyone clicking.
- For URLs already short (< 50 chars), there's no value in shortening; tell the user.
- Confirm with the user that the short URL works before reporting "done" — paste the original and the shortened side by side.

## Failure modes

- `Error: Please enter a valid URL` → the target URL is malformed or rejected (common for `http://localhost/...`).
- `Error: Custom URL already taken` (is.gd) → pick a different slug or drop the custom one.
- Silence / timeout → the service may be rate-limiting you. Back off several seconds.

## Paid alternatives with analytics

If the user wants click tracking, destination editing, or branded short domains, they'll need:
- Bitly (free tier with key, paid for custom domains) — `BITLY_ACCESS_TOKEN`.
- Short.io — custom domains + stats, has a free tier.

For public ad-hoc sharing, the no-key services above are fine.
