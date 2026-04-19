---
name: gifs
description: Find GIFs via Giphy or Tenor public APIs. Use when the user asks for a reaction gif, visual, or meme.
always: false
---

# gifs

Two sources. Giphy is the recommended path for new users; Tenor support is kept for people who already have a key.

## Giphy (recommended)

### Setup

1. Visit https://developers.giphy.com and click **Create An App**.
2. A public beta key is issued immediately — free, rate-limited. Upgrade to Production inside the dashboard when the usage grows.
3. Save it: `export GIPHY_API_KEY=...` in shell rc or the agent's config.

### Endpoint

```
GET https://api.giphy.com/v1/gifs/search?api_key=<KEY>&q=<QUERY>&limit=<N>&rating=g
```

Useful params:
- `q` — search string.
- `limit` — 1 to 50.
- `rating` — `g`, `pg`, `pg-13`, `r`. Default to `g` unless the user asks otherwise.
- `lang` — 2-letter code (default `en`).

Response shape: `data[].images.original.url` is the main GIF URL; `data[].images.fixed_height.url` is a chat-friendly 200px-tall version.

## Tenor (deprecated for new clients)

**Important:** as of January 2026 Google stopped accepting new Tenor API clients. Existing keys are expected to keep working for a limited grace period. For a new setup today, use Giphy. If the user already has a Tenor key and wants to keep using it:

```
GET https://tenor.googleapis.com/v2/search?q=<QUERY>&key=<KEY>&client_key=<app_name>&limit=<N>
```

Useful params:
- `client_key` — identifies your app (any short string you pick).
- `limit` — up to 50.
- `contentfilter` — `off`, `low`, `medium`, `high`. Default to `medium`.
- `media_filter` — comma-separated, e.g. `gif,tinygif,mp4`.

Env var: `TENOR_API_KEY`.

Response: `results[].media_formats.gif.url` for full-size, `results[].media_formats.tinygif.url` for small.

## Runtime source selection

```
if $GIPHY_API_KEY set  -> use Giphy
elif $TENOR_API_KEY set -> use Tenor
else                   -> tell the user no gif key is configured, and explain how to get a free Giphy key
```

## Presenting results

- In chat channels that embed URLs (Telegram, Discord, Slack), send the URL directly — the client renders the gif inline.
- Offer two or three options from near the top of the result list, not just the first hit. The first is often not the funniest.
- Match the query to intent: "something funny", "celebrating", "confused face". The search string matters.

## Failure modes

- `401 Unauthorized` → bad or revoked key.
- Empty `data` / `results` array → query matched nothing. Drop a word or try a broader term.
- Slow response → image CDN is sometimes throttled; URLs still work, rendering is just slow.
- Too-suggestive content surfacing → raise `rating` (Giphy) or `contentfilter` (Tenor).

## Rules

- Default to family-friendly rating/filter unless the user explicitly asks otherwise.
- Don't download the gif and re-host it. The CDN URL is the intended delivery path.
- Don't cache gif URLs across sessions. Trending changes, and the CDN path may rotate.
