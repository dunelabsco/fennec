---
name: rss
description: Poll RSS / Atom feeds for new items and surface updates. Use when the user wants to follow blog posts, podcast episodes, release notes, or any time-ordered publication.
always: false
---

# rss

No dedicated feed-reader tool in Fennec — fetch the feed URL with `web_fetch` and parse the text. Combine with `memory_store` to track "last seen" state so you only surface new items.

## Fetching

```
web_fetch(url: "https://example.com/feed.xml")
```

Atom and RSS are both XML. Most modern blogs use one or the other; the skill handles both.

## RSS 2.0 shape

Items live in `<channel>/<item>`. Per item:

```xml
<item>
  <title>…</title>
  <link>https://…</link>
  <description>…</description>
  <pubDate>Tue, 15 Apr 2026 10:00:00 GMT</pubDate>
  <guid>…</guid>
</item>
```

## Atom shape

Items live in `<feed>/<entry>`. Per entry:

```xml
<entry>
  <title>…</title>
  <link href="https://…"/>
  <summary>…</summary>
  <published>2026-04-15T10:00:00Z</published>
  <id>…</id>
</entry>
```

Key difference: Atom uses `<link href="...">`, RSS uses `<link>...</link>` (text content).

## Parsing

Treat the response as text and pattern-match tag pairs. No full XML parser needed for typical feeds. Extract `title`, `link`, `published` (or `pubDate`), and a short `summary`/`description`.

## Tracking "new since last check"

1. On first poll, save the set of `guid` / `id` / `link` values seen, plus a timestamp, via `memory_store` under a key like `rss:<feed_url>:seen`.
2. On subsequent polls, fetch the feed, compare item IDs against the stored set, surface only the new ones, and update the stored set.
3. For very active feeds, bound the stored set (e.g. keep the last 200 IDs) so memory doesn't grow forever.

## Polling schedule

Use the `cron` tool for periodic polling — e.g. every 30 minutes:
```
cron(schedule: "*/30 * * * *", prompt: "Check the <feed_url> RSS feed and report new items.")
```

The `cron` origin routes results back to the channel the user asked from.

## Tips

- Respect the feed's `<ttl>` (RSS) or `<updated>` cadence hint. Don't poll faster than the feed updates.
- Feed URLs sometimes live at `/feed`, `/rss`, `/atom.xml`, `/index.xml`, or `/.rss` — check the site's HTML `<link rel="alternate" type="application/rss+xml">` tag if not obvious.
- Some feeds are truncated (last 10 items only) — if the user wants deep history, point them at the archive, not the feed.
- Podcast feeds are RSS with extra `<enclosure url="...">` elements for audio files.

## Failure modes

- HTML page returned instead of feed → URL is wrong; look for the site's `<link rel="alternate">` or try common paths.
- 403 / 406 → some feeds block generic user-agents. Set `User-Agent: fennec/0.1`.
- Feed returns 200 but contents unchanged and no `pubDate` / `<updated>` → fall back to comparing item IDs.
- Encoding issues (mojibake in titles) → check the `<?xml encoding="...">` declaration and decode accordingly.
